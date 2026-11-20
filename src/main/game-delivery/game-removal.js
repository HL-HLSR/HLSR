"use strict";

// Electron's patched fs treats paths ending in *.asar as virtual archives.
// Removal must operate on the real files instead.
const fs = require("../launcher-update/fs-compat");
const path = require("path");

const libraryCleanup = require("../uninstall/library-cleanup");
const { DeliveryError } = require("./errors");
const { assertInside, isInside, gameInstallPath } = require("./paths");

const INSTALLED_FILE = "installed.json";
const GAME_ID_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

function treeSize(target) {
  let stat;
  try {
    stat = fs.lstatSync(target);
  } catch (e) {
    return 0;
  }

  if (stat.isSymbolicLink()) return 0;
  if (!stat.isDirectory()) return stat.size;

  let entries = [];
  try {
    entries = fs.readdirSync(target);
  } catch (e) {
    return 0;
  }

  let total = 0;
  for (const entry of entries) total += treeSize(path.join(target, entry));
  return total;
}

function exists(target) {
  try {
    fs.lstatSync(target);
    return true;
  } catch (e) {
    return false;
  }
}

function entryMatchers(gameId) {
  return [
    new RegExp(`^${gameId}\\.[^.]+\\.json$`),
    new RegExp(`^${gameId}-[0-9a-z]+-[0-9a-f]{8}$`),
    new RegExp(`^repair-mirror-${gameId}-[0-9a-z]+$`),
  ];
}

function ownEntries(zone, gameId) {
  let names = [];
  try {
    names = fs.readdirSync(zone);
  } catch (e) {
    return [];
  }

  const matchers = entryMatchers(gameId);

  return names
    .filter((name) => matchers.some((matcher) => matcher.test(name)))
    .map((name) => path.join(zone, name));
}

function targetsFor(paths, gameId) {
  const targets = [
    { kind: "game", zone: paths.games, path: path.join(paths.games, gameId) },
    { kind: "downloads", zone: paths.downloads, path: path.join(paths.downloads, gameId) },
    {
      kind: "manifests",
      zone: path.join(paths.internal, "manifests"),
      path: path.join(paths.internal, "manifests", gameId),
    },
    // Эпизоды, отложенные на время подмены сборки (InstallService.parkEpisodes).
    {
      kind: "state",
      zone: path.join(paths.state, "episodes-park"),
      path: path.join(paths.state, "episodes-park", gameId),
    },
  ];

  for (const [kind, zone] of [
    ["state", paths.state],
    ["staging", paths.staging],
    ["rollback", paths.rollback],
  ])
    for (const entry of ownEntries(zone, gameId)) targets.push({ kind, zone, path: entry });

  return targets;
}

function assertRemovable(paths, rawGameId) {
  const gameId = String(rawGameId || "");

  if (!GAME_ID_RE.test(gameId))
    throw new DeliveryError("PATH_INVALID", `Недопустимый gameId: ${rawGameId}`);

  const installPath = gameInstallPath(paths, gameId);

  if (!fs.existsSync(path.join(installPath, INSTALLED_FILE)))
    throw new DeliveryError("NOT_INSTALLED", "Игра не установлена", { gameId });

  const reparse = libraryCleanup.findReparsePoint(installPath);
  if (reparse)
    throw new DeliveryError(
      "PATH_INVALID",
      `В пути установки есть символьная ссылка или junction: ${reparse}`,
      { gameId, path: reparse }
    );

  return { gameId, installPath };
}

function planRemoval(paths, rawGameId) {
  const { gameId, installPath } = assertRemovable(paths, rawGameId);

  const targets = targetsFor(paths, gameId)
    .filter((target) => exists(target.path))
    .map((target) => Object.assign({}, target, { bytes: treeSize(target.path) }));

  return {
    gameId,
    installPath,
    targets,
    totalBytes: targets.reduce((sum, target) => sum + target.bytes, 0),
  };
}

function removeGame(paths, rawGameId) {
  const plan = planRemoval(paths, rawGameId);
  const result = { removed: 0, failed: [] };

  const installedFile = path.join(plan.installPath, INSTALLED_FILE);

  try {
    fs.unlinkSync(installedFile);
    result.removed += 1;
  } catch (error) {
    if (error.code !== "ENOENT")
      result.failed.push({ path: installedFile, error: error.code || error.message });
  }

  for (const target of plan.targets) {
    const zone = path.resolve(target.zone);
    const victim = path.resolve(target.path);

    if (victim === zone || !isInside(zone, victim)) {
      result.failed.push({ path: victim, error: "OUT_OF_ZONE" });
      continue;
    }

    assertInside(zone, victim, `Удаляемый путь (${target.kind})`);
    libraryCleanup.removeTreeNoFollow(victim, result);
  }

  return {
    removed: result.removed,
    failed: result.failed,
    partial: result.failed.length > 0,
    freedBytes: plan.totalBytes,
  };
}

module.exports = {
  GAME_ID_RE,
  targetsFor,
  assertRemovable,
  planRemoval,
  removeGame,
  treeSize,
  ownEntries,
};
