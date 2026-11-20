"use strict";

const fs = require("fs");
const fsp = require("fs/promises");
const path = require("path");
const { EventEmitter } = require("events");

const fileOps = require("./file-ops");
const { matchesMutable } = require("./mutable-paths");
const userConfigs = require("./user-configs");
const { createMatcher } = require("./path-match");
const { DeliveryError } = require("./errors");

const YIELD_EVERY = 25;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms || 0));
}

class IntegrityService extends EventEmitter {
  constructor(options) {
    const opts = options || {};
    super();

    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.busy = false;
    this.cancelRequested = false;
  }

  cancel() {
    if (!this.busy) return false;
    this.cancelRequested = true;
    return true;
  }

  async check(request) {
    if (this.busy) throw new DeliveryError("ALREADY_RUNNING", "Проверка файлов уже выполняется");

    const installPath = request.installPath;
    const manifest = request.filesManifest;

    if (!manifest || !Array.isArray(manifest.files))
      throw new DeliveryError("MANIFEST_INVALID", "Нет файлового манифеста для проверки");

    if (!fs.existsSync(installPath))
      throw new DeliveryError("NOT_INSTALLED", `Папка игры не найдена: ${installPath}`);

    const preserved = createMatcher(request.preservePaths || []);
    const gameId = request.gameId || manifest.gameId;

    this.busy = true;
    this.cancelRequested = false;

    const result = {
      gameId,
      version: manifest.version,
      total: 0,
      checked: 0,
      valid: [],
      missing: [],
      corrupted: [],
      sizeMismatch: [],
      // Стоковые cfg сборки: пропавший восстанавливается как повреждение,
      // изменённый — правка игрока (user-configs.js), его меняют только по выбору.
      configMissing: [],
      configChanged: [],
      extra: [],
      skipped: [],
    };

    try {
      // Стоковые cfg из папок игрока (files-manifest.js, stockConfigs) сверяются
      // только по запросу «Проверить файлы» и ремонта. Установка и обновление по
      // ним не судят: там такие cfg остаются у игрока как есть.
      const stock = request.stockConfigs && Array.isArray(manifest.stockConfigs) ? manifest.stockConfigs : [];
      const targets = manifest.files.filter((file) => {
        if (preserved(file.path) || matchesMutable(file.path)) {
          result.skipped.push(file.path);
          return false;
        }
        return true;
      }).concat(stock);

      result.total = targets.length;
      this.emit("progress", { gameId, checked: 0, total: result.total, phase: "size" });

      for (let index = 0; index < targets.length; index++) {
        if (this.cancelRequested) throw new DeliveryError("CANCELLED", "Проверка отменена");

        const file = targets[index];
        const full = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

        let stats = null;
        try {
          stats = await fsp.stat(full);
        } catch (e) {
          stats = null;
        }

        const config = userConfigs.isConfigPath(file.path);
        if (!stats || !stats.isFile()) (config ? result.configMissing : result.missing).push(file.path);
        else if (stats.size !== file.size) {
          if (config) result.configChanged.push(file.path);
          else result.sizeMismatch.push({ path: file.path, expected: file.size, actual: stats.size });
        } else {
          const digest = await fileOps.hashFile(full);
          if (digest === file.sha256) result.valid.push(file.path);
          else if (config) result.configChanged.push(file.path);
          else result.corrupted.push({ path: file.path, expected: file.sha256, actual: digest });
        }

        result.checked = index + 1;

        if (index % 5 === 0 || index === targets.length - 1)
          this.emit("progress", {
            gameId,
            checked: result.checked,
            total: result.total,
            percent: result.total ? (result.checked / result.total) * 100 : 100,
            phase: "hash",
          });

        if (index % YIELD_EVERY === 0) await delay(0);
      }

      result.extra = await this.findExtraFiles(installPath, manifest, preserved);
      // Повреждение — только файлы сборки из files. Стоковых cfg из папок игрока
      // может не быть у установки из базового архива: их предлагают докачать, но
      // игра от этого не «повреждена» (иначе круг «переустанови → снова повреждено»).
      const stockPaths = new Set(stock.map((file) => file.path));
      result.stockMissing = result.configMissing.filter((relative) => stockPaths.has(relative)).length;
      result.damaged =
        result.missing.length +
        result.corrupted.length +
        result.sizeMismatch.length +
        result.configMissing.length -
        result.stockMissing;
      result.ok = result.damaged === 0;

      this.logger.info(
        `[game-delivery] проверка файлов ${gameId}: проверено ${result.checked}, ` +
          `повреждено ${result.damaged}, стоковых cfg нет ${result.stockMissing}, ` +
          `изменённых cfg ${result.configChanged.length}, лишних ${result.extra.length}`
      );

      return result;
    } finally {
      this.busy = false;
      this.cancelRequested = false;
    }
  }

  async findExtraFiles(installPath, manifest, preserved) {
    const known = new Set(manifest.files.map((file) => file.path.toLowerCase()));
    const extra = [];
    const stack = [installPath];
    let steps = 0;

    while (stack.length) {
      const dir = stack.pop();
      let items;

      try {
        items = await fsp.readdir(dir, { withFileTypes: true });
      } catch (e) {
        continue;
      }

      for (const item of items) {
        const full = path.join(dir, item.name);
        const relative = path.relative(installPath, full).replace(/\\/g, "/");

        if (item.isSymbolicLink()) continue;
        if (preserved(relative) || matchesMutable(relative)) continue;

        if (item.isDirectory()) {
          stack.push(full);
          continue;
        }

        if (relative === "installed.json") continue;
        if (!known.has(relative.toLowerCase())) extra.push(relative);
      }

      if (++steps % YIELD_EVERY === 0) await delay(0);
    }

    return extra.sort();
  }
}

module.exports = IntegrityService;
