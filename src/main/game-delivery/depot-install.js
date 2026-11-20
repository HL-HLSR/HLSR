"use strict";

const fs = require("fs");
const path = require("path");

const manifestRegistry = require("./manifests");
const { DeliveryError, preserveError } = require("./errors");
const { assertInside, removeWithinZone, gameInstallPath } = require("./paths");
const { writeJsonAtomic } = require("./atomic");
const mutablePaths = require("./mutable-paths");
const playerConfig = require("./player-config");
const { createMatcher } = require("./path-match");

const INSTALLED_FILE = "installed.json";
const INSTALLED_SCHEMA_VERSION = 1;

const REQUIRED_FILES = ["hl2.exe", "hl2/gameinfo.txt"];

const SPEEDRUNSETUP_DXLEVEL = 0x5f;

function mergeInto(source, target) {
  let entries = [];
  try {
    entries = fs.readdirSync(source, { withFileTypes: true });
  } catch (error) {
    return 0;
  }

  let moved = 0;
  fs.mkdirSync(target, { recursive: true });

  for (const entry of entries) {
    const from = path.join(source, entry.name);
    const to = path.join(target, entry.name);

    if (entry.isSymbolicLink()) continue;

    if (entry.isDirectory()) {
      moved += mergeInto(from, to);
      continue;
    }

    fs.mkdirSync(path.dirname(to), { recursive: true });
    fs.renameSync(from, to);
    moved += 1;
  }

  return moved;
}

function existsInsensitive(root, relative) {
  const parts = relative.split("/");
  let current = root;

  for (const part of parts) {
    let names = [];
    try {
      names = fs.readdirSync(current);
    } catch (error) {
      return false;
    }

    const found = names.find((name) => name.toLowerCase() === part.toLowerCase());
    if (!found) return false;

    current = path.join(current, found);
  }

  return true;
}

class DepotInstaller {
  constructor(options) {
    const opts = options || {};

    this.paths = opts.paths;
    this.depotService = opts.depotService;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.setRegistryValue = opts.setRegistryValue || null;
    this.now = opts.now || (() => new Date().toISOString());
  }

  stagingFor(gameId) {
    const name = `${gameId}-depot-${Date.now().toString(36)}`;
    return assertInside(this.paths.staging, path.join(this.paths.staging, name), "Папка staging");
  }

  async install(gameId, options) {
    const opts = options || {};
    const installPath = gameInstallPath(this.paths, gameId);
    const staging = this.stagingFor(gameId);

    fs.mkdirSync(staging, { recursive: true });

    try {
      const downloaded = await this.depotService.download(gameId, staging, {
        libraryRoot: this.paths.root,
        onProgress: opts.onProgress,
      });

      const content = path.join(staging, "content");
      let files = 0;
      for (const depot of downloaded.depots) files += mergeInto(depot.dir, content);

      this.logger.info(`[depot-install] ${gameId}: собрано ${files} файлов из ${downloaded.depots.length} депотов`);

      for (const relative of REQUIRED_FILES)
        if (!existsInsensitive(content, relative))
          throw new DeliveryError(
            "REQUIRED_FILE_MISSING",
            `Steam не отдал обязательный файл сборки: ${relative}`,
            { gameId, file: relative }
          );

      this.applyPostInstall(content, gameId);

      fs.mkdirSync(path.dirname(installPath), { recursive: true });

      let previous = null;
      if (fs.existsSync(installPath)) {
        previous = `${installPath}.hlsr-backup`;
        if (fs.existsSync(previous)) removeWithinZone(this.paths.games, previous);

        this.preserveUserData(installPath, content, gameId);

        fs.renameSync(installPath, previous);
      }

      try {
        fs.renameSync(content, installPath);
      } catch (error) {
        if (previous) fs.renameSync(previous, installPath);
        throw error;
      }

      const installed = {
        schemaVersion: INSTALLED_SCHEMA_VERSION,
        gameId,
        version: opts.version || "1.0.0",
        installedAt: this.now(),
        publishedAt: null,
        manifestSha256: null,
        installPath,
        archiveSha256: null,
        source: "steam-depot",
        state: "ready",
      };

      writeJsonAtomic(path.join(installPath, INSTALLED_FILE), installed);

      if (previous)
        try {
          removeWithinZone(this.paths.games, previous);
        } catch (error) {
          this.logger.warn(
            `[depot-install] прежняя версия осталась на диске: ${error.message}`
          );
        }

      this.logger.info(`[depot-install] ${gameId} установлена из депотов Steam`);

      return installed;
    } finally {
      try {
        if (fs.existsSync(staging)) removeWithinZone(this.paths.staging, staging);
      } catch (error) {
        this.logger.warn(`[depot-install] staging не убран: ${error.message}`);
      }
    }
  }

  preserveUserData(installPath, content, gameId) {
    const matches = createMatcher(mutablePaths.USER_DATA);
    const skipDir = createMatcher(mutablePaths.LIBRARY_INTERNALS);

    let copied = 0;

    const walk = (dir, relative) => {
      let entries;
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch (error) {
        return;
      }

      for (const entry of entries) {
        const rel = relative ? `${relative}/${entry.name}` : entry.name;

        if (entry.isSymbolicLink()) continue;

        if (entry.isDirectory()) {
          if (!skipDir(rel)) walk(path.join(dir, entry.name), rel);
          continue;
        }

        if (!matches(rel)) continue;

        const target = assertInside(
          content,
          path.join(content, rel.replace(/\//g, path.sep)),
          `Пользовательский файл "${rel}"`
        );
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.copyFileSync(path.join(dir, entry.name), target);
        copied++;
      }
    };

    // Как InstallService: не скопировался хоть один файл игрока — прерываемся
    // до подмены папок, пока прежняя сборка цела.
    try {
      walk(installPath, "");
    } catch (error) {
      throw preserveError(error);
    }

    if (copied) this.logger.info(`[depot-install] ${gameId}: перенесено пользовательских файлов ${copied}`);
  }

  applyPostInstall(content, gameId) {
    const cfgDir = path.join(content, "hl2", "cfg");

    try {
      fs.mkdirSync(cfgDir, { recursive: true });
      fs.writeFileSync(path.join(cfgDir, "autoexec.cfg"), playerConfig.template(gameId), "utf8");
    } catch (error) {
      this.logger.warn(`[depot-install] autoexec.cfg не записан: ${error.message}`);
    }

    const launch = manifestRegistry.launchConfigOf(gameId);

    if (launch && launch.steamAppId) {
      try {
        fs.writeFileSync(path.join(content, "steam_appid.txt"), `${launch.steamAppId}\n`, "utf8");
      } catch (error) {
        this.logger.warn(`[depot-install] steam_appid.txt не записан: ${error.message}`);
      }
    }

    if (!this.setRegistryValue) return;

    try {
      this.setRegistryValue(
        "HKCU\\Software\\Valve\\Source\\hl2\\Settings",
        "DXLevel_V1",
        SPEEDRUNSETUP_DXLEVEL
      );
    } catch (error) {
      this.logger.warn(`[depot-install] ключ DXLevel не установлен: ${error.message}`);
    }
  }
}

module.exports = DepotInstaller;
module.exports.REQUIRED_FILES = REQUIRED_FILES;
module.exports.mergeInto = mergeInto;
module.exports.existsInsensitive = existsInsensitive;
