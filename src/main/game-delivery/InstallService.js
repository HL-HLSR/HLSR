"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { EventEmitter } = require("events");

const ArchiveService = require("./ArchiveService");
const fileOps = require("./file-ops");
const buildMetadataTrust = require("./build-metadata-trust");
const { readJsonSafe, readJsonState, writeJsonAtomic } = require("./atomic");
const { DeliveryError, toDeliveryError, isAntivirusBlock, antivirusError, preserveError } = require("./errors");
const { assertInside, removeWithinZone, gameInstallPath } = require("./paths");
const mutablePaths = require("./mutable-paths");
const episodes = require("./episodes");
const { createMatcher } = require("./path-match");
const userConfigs = require("./user-configs");
const { treeSize } = require("./game-removal");

const INSTALLED_SCHEMA_VERSION = 1;
const OPERATION_SCHEMA_VERSION = 1;
const INSTALLED_FILE = "installed.json";
const LOCK_FILE = "install.lock";
const SPACE_MARGIN = 512 * 1024 * 1024;

const ORPHAN_STAGING_MIN_AGE_MS = 24 * 60 * 60 * 1000;

const UPDATE_SWAP_PHASES = ["swap-retire", "swap-place", "installed-write", "commit-complete"];

const UPDATE_FAULTS = [
  "before-retire",
  "after-retire",
  "before-place",
  "after-place",
  "before-installed-write",
  "after-installed-write",
];

function nowIso() {
  return new Date().toISOString();
}

const { freeSpaceBytes } = require("./free-space");

function processAlive(pid) {
  if (!pid) return false;

  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === "EPERM";
  }
}

class InstallService extends EventEmitter {
  constructor(options) {
    super();
    const opts = options || {};

    this.paths = opts.paths;
    this.archiveService = opts.archiveService || new ArchiveService({ logger: opts.logger });
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.spaceMargin = opts.spaceMargin == null ? SPACE_MARGIN : opts.spaceMargin;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.metadataNow = opts.metadataNow || null;

    this.faultInjector = opts.faultInjector || null;

    this.current = null;
  }

  fault(name, operation) {
    if (this.faultInjector) this.faultInjector(name, operation);
  }

  isBusy() {
    return !!this.current;
  }

  lockPath() {
    return path.join(this.paths.state, LOCK_FILE);
  }

  acquireLock(gameId) {
    const file = this.lockPath();

    if (fs.existsSync(file)) {
      let existing = null;
      try {
        existing = JSON.parse(fs.readFileSync(file, "utf8"));
      } catch (e) {
        existing = null;
      }

      if (existing && processAlive(existing.pid) && existing.pid !== process.pid)
        throw new DeliveryError("ALREADY_RUNNING", "Другая установка уже выполняется", {
          gameId: existing.gameId,
          pid: existing.pid,
        });

      if (existing) this.logger.warn("[game-delivery] найдена устаревшая блокировка установки, снимаю");
    }

    fs.writeFileSync(
      file,
      JSON.stringify({ pid: process.pid, gameId, host: os.hostname(), startedAt: nowIso() }, null, 2),
      "utf8"
    );
  }

  releaseLock() {
    const file = this.lockPath();
    try {
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch (e) {
      this.logger.warn(`[game-delivery] не удалось снять блокировку: ${e.message}`);
    }
  }

  operationPath(gameId) {
    return path.join(this.paths.state, `${gameId}.operation.json`);
  }

  readOperation(gameId) {
    const data = readJsonSafe(this.operationPath(gameId));
    if (!data || data.version !== OPERATION_SCHEMA_VERSION) return null;
    return data;
  }

  writeOperation(operation) {
    fs.mkdirSync(this.paths.state, { recursive: true });
    fs.writeFileSync(
      this.operationPath(operation.gameId),
      JSON.stringify(Object.assign({ version: OPERATION_SCHEMA_VERSION }, operation), null, 2),
      "utf8"
    );
  }

  updateOperation(operation, patch) {
    Object.assign(operation, patch, { updatedAt: nowIso() });
    this.writeOperation(operation);
  }

  clearOperation(gameId) {
    const file = this.operationPath(gameId);
    try {
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch (e) {
      this.logger.warn(`[game-delivery] не удалось удалить журнал операции: ${e.message}`);
    }
  }

  listPendingOperations() {
    let files = [];
    try {
      files = fs.readdirSync(this.paths.state).filter((name) => name.endsWith(".operation.json"));
    } catch (e) {
      return [];
    }

    const pending = [];

    for (const name of files) {
      let data;
      try {
        data = JSON.parse(fs.readFileSync(path.join(this.paths.state, name), "utf8"));
      } catch (e) {
        continue;
      }

      const stagingExists = !!data.stagingPath && fs.existsSync(data.stagingPath);
      pending.push({
        gameId: data.gameId,
        kind: data.kind || "install",
        phase: data.phase,
        fromVersion: data.fromVersion || null,
        toVersion: data.toVersion || null,
        stagingPath: data.stagingPath || null,
        stagingExists,
        archivePath: data.archivePath || null,
        archiveExists: !!data.archivePath && fs.existsSync(data.archivePath),
        startedAt: data.startedAt,
        updatedAt: data.updatedAt,
      });
    }

    return pending;
  }

  cleanupPending(gameId) {
    const raw = this.readOperation(gameId);
    if (raw && raw.kind === "full-update") return this.recoverFullUpdate(gameId);

    const operation = this.listPendingOperations().find((item) => item.gameId === gameId);
    if (!operation) return { removed: false };

    if (operation.stagingPath && fs.existsSync(operation.stagingPath))
      removeWithinZone(this.paths.staging, operation.stagingPath);

    this.clearOperation(gameId);
    return { removed: true };
  }

  cleanupOrphanStaging(options) {
    const opts = options || {};
    const minAgeMs = opts.minAgeMs == null ? ORPHAN_STAGING_MIN_AGE_MS : opts.minAgeMs;
    const now = opts.now || Date.now();

    if (this.current) return 0;

    let names = [];
    try {
      names = fs.readdirSync(this.paths.staging);
    } catch (e) {
      return 0;
    }

    const known = this.stagingNamesInUse();
    let removed = 0;

    for (const name of names) {
      if (known.has(name)) continue;

      const target = path.join(this.paths.staging, name);

      try {
        if (now - fs.statSync(target).mtimeMs < minAgeMs) continue;
      } catch (e) {
        continue;
      }

      try {
        removeWithinZone(this.paths.staging, target);
        removed++;
      } catch (error) {
        this.logger.warn(`[game-delivery] не удалось убрать забытый staging ${name}: ${error.message}`);
      }
    }

    if (removed) this.logger.info(`[game-delivery] убрано забытых каталогов staging: ${removed}`);

    return removed;
  }

  stagingNamesInUse() {
    const known = new Set();

    let files = [];
    try {
      files = fs.readdirSync(this.paths.state).filter((name) => name.endsWith(".json"));
    } catch (e) {
      return known;
    }

    for (const name of files) {
      const data = readJsonSafe(path.join(this.paths.state, name));
      if (!data) continue;

      for (const value of [data.stagingPath, data.rollbackPath, data.mirrorPath])
        if (typeof value === "string" && value) known.add(path.basename(value));
    }

    return known;
  }

  readInstalled(gameId) {
    const installPath = gameInstallPath(this.paths, gameId);
    const file = path.join(installPath, INSTALLED_FILE);

    try {
      const data = JSON.parse(fs.readFileSync(file, "utf8"));
      if (data.schemaVersion !== INSTALLED_SCHEMA_VERSION || data.state !== "ready") return null;
      // Игра там, где лежит её installed.json. Записанный при установке путь
      // устаревает, если библиотеку перенесли или скопировали, и тогда
      // лаунчер запускал бы игру (и писал её конфиги) в старой папке.
      data.installPath = installPath;
      return data;
    } catch (e) {
      return null;
    }
  }

  fillInstalled(gameId, patch) {
    const PROTECTED = [
      "schemaVersion",
      "gameId",
      "version",
      "state",
      "installPath",
      "manifestSha256",
      "archiveSha256",
      "installedAt",
      "buildMetadata",
      "buildMetadataSha256",
    ];

    const current = this.readInstalled(gameId);
    if (!current) return null;

    const additions = {};
    for (const key of Object.keys(patch || {})) {
      if (PROTECTED.indexOf(key) !== -1) continue;
      if (current[key] != null) continue;
      if (patch[key] == null) continue;
      additions[key] = patch[key];
    }

    if (!Object.keys(additions).length) return current;

    const updated = Object.assign({}, current, additions);
    writeJsonAtomic(path.join(current.installPath, INSTALLED_FILE), updated);

    return updated;
  }

  replaceBuildMetadataSnapshot(gameId, snapshot) {
    const current = this.readInstalled(gameId);
    if (!current) return null;
    if (!snapshot || !snapshot.buildMetadata) return null;

    const updated = Object.assign({}, current, {
      buildMetadata: snapshot.buildMetadata,
      buildMetadataSha256: snapshot.buildMetadataSha256 || null,
      buildMetadataVerifiedAt: snapshot.buildMetadataVerifiedAt || null,
    });

    writeJsonAtomic(path.join(current.installPath, INSTALLED_FILE), updated);

    return updated;
  }

  cancel() {
    if (!this.current) return false;
    this.current.cancelRequested = true;
    if (this.current.cancelExtract) this.current.cancelExtract();
    return true;
  }

  async install(request) {
    if (this.current)
      throw new DeliveryError("ALREADY_RUNNING", "Установка уже выполняется", {
        gameId: this.current.gameId,
      });

    const manifest = request.manifest;
    const gameId = manifest.gameId;
    const installPath = gameInstallPath(this.paths, gameId);

    const update = request.update || null;

    const stagingName = `${gameId}-${Date.now().toString(36)}-${crypto
      .randomBytes(4)
      .toString("hex")}`;
    const stagingPath = assertInside(
      this.paths.staging,
      path.join(this.paths.staging, stagingName),
      "Папка staging"
    );

    const ctx = {
      gameId,
      manifest,
      manifestSha256: request.manifestSha256,
      archivePath: request.archivePath,
      installPath,
      stagingPath,
      update,
      filesManifest: request.filesManifest || (update && update.filesManifest) || null,
      publishedAt: request.publishedAt || manifest.publishedAt || null,
      buildMetadataSnapshot: null,
      rollbackPath: update
        ? assertInside(this.paths.rollback, path.join(this.paths.rollback, stagingName), "Папка rollback")
        : null,
      cancelRequested: false,
      cancelExtract: null,
    };

    const operation = {
      gameId,
      kind: update ? "full-update" : "install",
      fromVersion: update ? update.fromVersion : null,
      toVersion: manifest.version,
      phase: "starting",
      stagingPath,
      installPath,
      rollbackPath: ctx.rollbackPath,
      archivePath: ctx.archivePath,
      startedAt: nowIso(),
      updatedAt: nowIso(),
    };

    this.acquireLock(gameId);

    try {
      this.current = ctx;
      this.writeOperation(operation);

      const result = await this.runInstall(ctx, operation);
      this.clearOperation(gameId);
      return result;
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");
      this.recordFailure(ctx, operation, delivery);

      if (delivery.code === "ROLLBACK_FAILED") {
        this.emit("state", { gameId, state: "error", error: delivery.toPublic() });
        throw delivery;
      }

      try {
        if (fs.existsSync(ctx.stagingPath)) removeWithinZone(this.paths.staging, ctx.stagingPath);
        if (ctx.rollbackPath && fs.existsSync(ctx.rollbackPath))
          removeWithinZone(this.paths.rollback, ctx.rollbackPath);
        this.clearOperation(gameId);
      } catch (cleanupError) {
        this.logger.error(
          `[game-delivery] не удалось убрать staging ${ctx.stagingPath}: ${cleanupError.message}`
        );
      }

      this.emit("state", { gameId, state: delivery.code === "CANCELLED" ? "cancelled" : "error", error: delivery.toPublic() });
      throw delivery;
    } finally {
      this.releaseLock();
      this.current = null;
    }
  }

  async runInstall(ctx, operation) {
    const manifest = ctx.manifest;

    this.updateOperation(operation, { phase: "checking" });
    this.emit("state", { gameId: ctx.gameId, state: "preparing" });
    this.checkWritable();
    this.checkSpace(manifest.expectedInstalledSize, this.userDataBytes(ctx));
    this.throwIfCancelled(ctx);

    this.updateOperation(operation, { phase: "verifying" });
    this.emit("state", { gameId: ctx.gameId, state: "verifying" });
    const archiveSha256 = await this.verifyArchive(ctx);
    this.throwIfCancelled(ctx);

    this.updateOperation(operation, { phase: "inspecting" });
    const inspection = await this.archiveService.inspect(ctx.archivePath, {
      maxEntries: manifest.filesManifest.maxEntries,
      maxUncompressedSize: manifest.filesManifest.maxUncompressedSize,
      rootPrefix: manifest.baseArchive.rootPrefix || null,
    });

    this.checkStagingContainment(ctx, inspection.listing.entries);
    this.checkExpectedSize(manifest, inspection.report.uncompressedSize);
    this.throwIfCancelled(ctx);

    this.updateOperation(operation, { phase: "extracting" });
    this.emit("state", { gameId: ctx.gameId, state: "extracting", percent: 0 });
    fs.mkdirSync(ctx.stagingPath, { recursive: true });
    await this.extract(ctx);
    this.throwIfCancelled(ctx);

    this.updateOperation(operation, { phase: "validating" });
    const contentRoot = this.resolveContentRoot(ctx);
    this.checkRequiredFiles(manifest, contentRoot);
    this.throwIfCancelled(ctx);

    this.verifyBuildMetadata(ctx, contentRoot);
    this.throwIfCancelled(ctx);

    if (ctx.update) {
      this.assertStillInstalled(ctx);
      this.assertGameNotRunning(ctx);
      this.stripLibraryInternals(contentRoot);
      await this.verifyStagingTree(ctx, contentRoot);
      this.throwIfCancelled(ctx);
    }

    this.updateOperation(operation, { phase: "finalizing" });
    this.emit("state", { gameId: ctx.gameId, state: "finalizing" });
    this.preserveUserData(ctx, contentRoot);

    let installed;
    if (ctx.update) {
      installed = this.commitUpdate(ctx, operation, contentRoot, archiveSha256);
    } else {
      this.swapIntoLibrary(ctx, contentRoot);
      installed = this.writeInstalled(ctx, archiveSha256, ctx.buildMetadataSnapshot || undefined);
    }

    this.emit("state", { gameId: ctx.gameId, state: "ready", installed });

    return installed;
  }

  verifyBuildMetadata(ctx, contentRoot) {
    const descriptor = ctx.manifest.buildMetadata;
    if (!descriptor) return;

    try {
      const verified = buildMetadataTrust.verifyForInstall({
        installRoot: contentRoot,
        descriptor,
        manifest: Object.assign({}, ctx.manifest, {
          publishedAt: ctx.publishedAt || ctx.manifest.publishedAt || null,
        }),
        filesManifest: ctx.filesManifest,
        launcherVersion: this.launcherVersion,
        now: this.metadataNow || undefined,
      });
      ctx.buildMetadataSnapshot = buildMetadataTrust.snapshotFromVerified(verified);
    } catch (error) {
      if (descriptor.required === false && error.code === buildMetadataTrust.REASONS.MISSING) return;
      throw new DeliveryError(error.code, error.message, error.detail || null);
    }
  }

  assertStillInstalled(ctx) {
    const data = readJsonSafe(path.join(ctx.installPath, INSTALLED_FILE));

    if (!data || data.state !== "ready")
      throw new DeliveryError("NOT_INSTALLED", "Игра не установлена — обновлять нечего", {
        gameId: ctx.gameId,
      });

    if (data.version !== ctx.update.fromVersion)
      throw new DeliveryError(
        "VERSION_MISMATCH",
        `Обновление готовилось для версии ${ctx.update.fromVersion}, установлена ${data.version}`,
        { expected: ctx.update.fromVersion, installed: data.version }
      );
  }

  assertGameNotRunning(ctx) {
    const check = fileOps.isGameRunning(ctx.installPath, ["hl2.exe"]);

    if (check.running)
      throw new DeliveryError(
        "GAME_RUNNING",
        `Игра запущена (${check.path}). Закройте её и повторите обновление.`,
        { path: check.path }
      );
  }

  stripLibraryInternals(contentRoot) {
    let names = [];
    try {
      names = fs.readdirSync(contentRoot);
    } catch (e) {
      return;
    }

    for (const name of names) {
      if (name !== INSTALLED_FILE && !name.startsWith(`${INSTALLED_FILE}.`)) continue;

      this.logger.warn(`[game-delivery] в архиве оказался служебный файл ${name} — удаляю из staging`);
      fs.rmSync(path.join(contentRoot, name), { force: true });
    }
  }

  async verifyStagingTree(ctx, contentRoot) {
    const update = ctx.update;

    if (!update.filesManifest)
      throw new DeliveryError(
        "FILES_MANIFEST_MISSING",
        "Нет файлового манифеста целевой версии — сверить распакованную сборку нечем"
      );

    if (!update.integrityService)
      throw new DeliveryError("VERIFY_FAILED", "Не передан сервис проверки файлов");

    this.emit("state", { gameId: ctx.gameId, state: "verifying" });

    const result = await update.integrityService.check({
      gameId: ctx.gameId,
      installPath: contentRoot,
      filesManifest: update.filesManifest,
      preservePaths: ctx.manifest.preservePaths || [],
    });

    if (!result.ok) {
      const samples = result.missing
        .concat(result.corrupted.map((item) => item.path))
        .concat(result.sizeMismatch.map((item) => item.path))
        .slice(0, 10);

      throw new DeliveryError(
        "VERIFY_FAILED",
        `Распакованная сборка не совпала с манифестом версии ${update.filesManifest.version}: ` +
          `повреждено или отсутствует файлов — ${result.damaged}`,
        { damaged: result.damaged, samples }
      );
    }

    if (result.extra.length)
      throw new DeliveryError(
        "VERIFY_FAILED",
        `В архиве есть файлы, которых нет в манифесте версии ${update.filesManifest.version}: ` +
          result.extra.slice(0, 5).join(", "),
        { extra: result.extra.slice(0, 10) }
      );

    return result;
  }

  commitUpdate(ctx, operation, contentRoot, archiveSha256) {
    fs.mkdirSync(this.paths.games, { recursive: true });
    fs.mkdirSync(ctx.rollbackPath, { recursive: true });

    const retired = assertInside(
      this.paths.rollback,
      path.join(ctx.rollbackPath, "previous"),
      "Старая версия"
    );

    this.updateOperation(operation, { phase: "swap-retire", retiredPath: retired });
    this.fault("before-retire", operation);

    this.parkEpisodes(ctx.gameId, ctx.installPath);
    try {
      fs.renameSync(ctx.installPath, retired);
    } catch (error) {
      this.unparkEpisodes(ctx.gameId, ctx.installPath);
      throw error;
    }

    try {
      this.fault("after-retire", operation);

      this.updateOperation(operation, { phase: "swap-place" });
      this.fault("before-place", operation);

      fs.renameSync(contentRoot, ctx.installPath);

      try {
        this.fault("after-place", operation);

        this.updateOperation(operation, { phase: "installed-write" });
        this.fault("before-installed-write", operation);

        const installed = this.writeInstalled(
          ctx,
          archiveSha256,
          Object.assign(
            {
              migratedFrom: ctx.update.fromVersion,
              filesManifestSha256: ctx.update.filesManifest.manifestSha256 || null,
              totalFiles: ctx.update.filesManifest.totalFiles,
              totalSize: ctx.update.filesManifest.totalSize,
            },
            ctx.buildMetadataSnapshot || {}
          )
        );

        this.fault("after-installed-write", operation);
        this.updateOperation(operation, { phase: "commit-complete" });

        this.cleanupUpdateZones(ctx);
        this.unparkEpisodes(ctx.gameId, ctx.installPath);

        return installed;
      } catch (error) {
        this.discardPlacedTree(ctx.installPath, ctx.stagingPath);
        throw error;
      }
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");

      try {
        if (!fs.existsSync(ctx.installPath) && fs.existsSync(retired))
          fs.renameSync(retired, ctx.installPath);
      } catch (restoreError) {
        this.logger.error(
          `[game-delivery] откат обновления не удался: ${restoreError.message}. ` +
            `Старая версия сохранена в ${retired}`
        );

        throw new DeliveryError(
          "ROLLBACK_FAILED",
          `Обновление не применено и откат не удался: ${restoreError.message}. ` +
            `Файлы прежней версии сохранены в ${retired}`,
          { rollbackPath: ctx.rollbackPath, original: delivery.toPublic() }
        );
      }

      if (fs.existsSync(path.join(ctx.installPath, INSTALLED_FILE)))
        this.logger.warn(
          `[game-delivery] обновление ${ctx.gameId} прервано на фазе ${operation.phase}, ` +
            "старая версия возвращена на место"
        );

      this.unparkEpisodes(ctx.gameId, ctx.installPath);
      throw delivery;
    }
  }

  discardPlacedTree(installPath, stagingPath) {
    const discard = assertInside(
      this.paths.staging,
      path.join(this.paths.staging, `${path.basename(stagingPath)}-discard`),
      "Отменённая новая версия"
    );

    fs.renameSync(installPath, discard);
    removeWithinZone(this.paths.staging, discard);
  }

  // Эпизоды (mutable-paths EPISODE_DIRS, ~6 ГБ) на время подмены сборки откладываются в
  // .hlsr/state/episodes-park/<игра> — её не чистят ни откат, ни уборка staging — и
  // возвращаются rename в ту сборку, что стоит после подмены или отката.
  episodesParkPath(gameId) {
    return path.join(this.paths.state, "episodes-park", gameId);
  }

  parkEpisodes(gameId, installPath) {
    this.unparkEpisodes(gameId, installPath); // хвост прошлой подмены, прерванной крахом
    const park = this.episodesParkPath(gameId);
    const moved = [];
    try {
      for (const dir of mutablePaths.EPISODE_DIRS) {
        const source = path.join(installPath, dir);
        if (!fs.existsSync(source)) continue;
        const target = path.join(park, dir);
        // Отложенный раньше не вернулся (в игре уже был свой): он отходит в сторону, а не стопорит обновления.
        if (fs.existsSync(target)) {
          const aside = `${target}.stale-${Date.now().toString(36)}`;
          fs.renameSync(target, aside);
          this.logger.warn(`[game-delivery] прежняя отложенная копия эпизода ${dir} отложена в ${aside}`);
        }
        fs.mkdirSync(park, { recursive: true });
        fs.renameSync(source, target);
        moved.push(dir);
      }
    } catch (error) {
      for (const dir of moved) {
        try {
          fs.renameSync(path.join(park, dir), path.join(installPath, dir));
        } catch (back) {
          this.logger.error(`[game-delivery] эпизод ${dir} остался в ${park}: ${back.message}`);
        }
      }
      throw toDeliveryError(error, "INSTALL_FAILED");
    }
  }

  // Не вернулся (в сборке своя папка, файл занят) — остаётся отложенным, повтор на старте лаунчера.
  unparkEpisodes(gameId, installPath = gameInstallPath(this.paths, gameId)) {
    const park = this.episodesParkPath(gameId);
    if (!fs.existsSync(park)) return;
    for (const dir of mutablePaths.EPISODE_DIRS) {
      const source = path.join(park, dir);
      if (!fs.existsSync(source)) continue;
      const target = path.join(installPath, dir);
      // Встал эпизод, поставленный лаунчером (маркер внутри), — отложенная копия прежняя: из неё нужны
      // только данные игрока. Чужая папка (своя episodic в сборке) — копия ждёт отложенной целиком.
      if (fs.existsSync(path.join(target, episodes.MARKER))) {
        this.mergeParkedEpisode(source, target);
        continue;
      }
      if (!fs.existsSync(installPath) || fs.existsSync(target)) {
        this.logger.warn(`[game-delivery] эпизод ${dir} пока отложен в ${source}: в ${installPath} его некуда вернуть`);
        continue;
      }
      try {
        fs.renameSync(source, target);
      } catch (error) {
        this.logger.error(`[game-delivery] эпизод ${dir} не вернулся из ${source}: ${error.message}`);
      }
    }
    try {
      if (!fs.readdirSync(park).length) fs.rmdirSync(park);
    } catch (error) {
      /* пусто или нет — не важно */
    }
  }

  // Папка эпизода уже стоит (подмена эпизода прервалась после того, как встала новая): данные игрока
  // (episodes.PLAYER_ENTRIES, демо в корне) переезжают в неё rename, остальное — контент игры —
  // удаляется. Одноимённое в новой уже есть — копия игрока уходит в <dir>.player-<время>, а не
  // в корзину. Не перенеслось — отложенная копия остаётся целиком до следующего старта.
  mergeParkedEpisode(source, target) {
    try {
      const existing = new Set(fs.readdirSync(target).map((name) => name.toLowerCase()));
      let aside = null;
      for (const name of fs.readdirSync(source).filter(episodes.isPlayerEntry)) {
        if (!existing.has(name.toLowerCase())) {
          fs.renameSync(path.join(source, name), path.join(target, name));
          continue;
        }
        aside = aside || `${source}.player-${Date.now().toString(36)}`;
        fs.mkdirSync(aside, { recursive: true });
        fs.renameSync(path.join(source, name), path.join(aside, name));
        this.logger.warn(`[game-delivery] ${name} игрока уже есть в ${target}: отложенная копия — в ${aside}`);
      }
      fs.rmSync(source, { recursive: true, force: true });
    } catch (error) {
      this.logger.error(`[game-delivery] данные игрока из ${source} не перенесены в ${target}: ${error.message}`);
    }
  }

  cleanupUpdateZones(ctx) {
    for (const [zone, target] of [
      [this.paths.rollback, ctx.rollbackPath],
      [this.paths.staging, ctx.stagingPath],
    ]) {
      try {
        if (target && fs.existsSync(target)) removeWithinZone(zone, target);
      } catch (e) {
        this.logger.warn(`[game-delivery] не удалось убрать ${target}: ${e.message}`);
      }
    }
  }

  throwIfCancelled(ctx) {
    if (ctx.cancelRequested) throw new DeliveryError("CANCELLED", "Установка отменена");
  }

  checkWritable() {
    for (const dir of [this.paths.games, this.paths.staging, this.paths.state, this.paths.rollback]) {
      try {
        fs.mkdirSync(dir, { recursive: true });
        const probe = path.join(dir, `.hlsr-write-test-${process.pid}-${Date.now()}`);
        fs.writeFileSync(probe, "");
        fs.unlinkSync(probe);
      } catch (e) {
        throw new DeliveryError("PATH_NOT_WRITABLE", `Нет прав на запись в ${dir}: ${e.message}`);
      }
    }
  }

  // userDataBytes — копии файлов игрока, которые ляжут в новую сборку рядом со старой.
  checkSpace(expectedInstalledSize, userDataBytes) {
    if (!expectedInstalledSize)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        "В манифесте не указан expectedInstalledSize — проверка места невозможна"
      );

    const free = freeSpaceBytes(this.paths.staging);
    if (free == null) return;

    const required = expectedInstalledSize + (userDataBytes || 0) + this.spaceMargin;
    if (free < required)
      throw new DeliveryError(
        "NO_SPACE",
        `Недостаточно места: нужно ${required}, свободно ${free}`,
        { requiredBytes: required, freeBytes: free }
      );
  }

  async verifyArchive(ctx) {
    const manifest = ctx.manifest;

    let stats;
    try {
      stats = fs.statSync(ctx.archivePath);
    } catch (e) {
      throw new DeliveryError("SIZE_MISMATCH", `Архив не найден: ${ctx.archivePath}`);
    }

    if (stats.size !== manifest.baseArchive.size)
      throw new DeliveryError(
        "SIZE_MISMATCH",
        `Размер архива ${stats.size} не совпадает с манифестом ${manifest.baseArchive.size}`,
        { expected: manifest.baseArchive.size, actual: stats.size }
      );

    const digest = await this.hashFile(ctx, ctx.archivePath);

    if (digest !== manifest.baseArchive.sha256)
      throw new DeliveryError(
        "CHECKSUM_MISMATCH",
        "SHA-256 архива не совпадает с манифестом",
        { expected: manifest.baseArchive.sha256, actual: digest }
      );

    return digest;
  }

  hashFile(ctx, filePath) {
    return new Promise((resolve, reject) => {
      const hash = crypto.createHash("sha256");
      const stream = fs.createReadStream(filePath, { highWaterMark: 4 * 1024 * 1024 });

      stream.on("data", (chunk) => {
        if (ctx.cancelRequested) {
          stream.destroy();
          return;
        }
        hash.update(chunk);
      });

      stream.on("error", (error) =>
        reject(isAntivirusBlock(error)
          ? antivirusError(filePath, error)
          : new DeliveryError("INSTALL_FAILED", `Не удалось прочитать архив: ${error.message}`))
      );

      stream.on("close", () => {
        if (ctx.cancelRequested) reject(new DeliveryError("CANCELLED", "Установка отменена"));
      });

      stream.on("end", () => resolve(hash.digest("hex")));
    });
  }

  checkStagingContainment(ctx, entries) {
    for (const entry of entries) {
      const relative = String(entry.path).replace(/\\/g, path.sep);
      const target = path.resolve(ctx.stagingPath, relative);

      assertInside(ctx.stagingPath, target, `Элемент архива "${entry.path}"`);
    }
  }

  checkExpectedSize(manifest, uncompressedSize) {
    const expected = manifest.expectedInstalledSize;
    if (!expected) return;

    const tolerance = Math.max(16 * 1024 * 1024, Math.round(expected * 0.02));

    if (Math.abs(uncompressedSize - expected) > tolerance)
      throw new DeliveryError(
        "ARCHIVE_LIMIT_EXCEEDED",
        `Распакованный размер ${uncompressedSize} расходится с манифестом ${expected}`,
        { expected, actual: uncompressedSize, tolerance }
      );
  }

  async extract(ctx) {
    const job = this.archiveService.extract(ctx.archivePath, ctx.stagingPath, {
      onProgress: (fraction) =>
        this.emit("state", {
          gameId: ctx.gameId,
          state: "extracting",
          percent: Math.round(fraction * 100),
        }),
    });

    ctx.cancelExtract = job.cancel;

    try {
      await job.promise;
    } finally {
      ctx.cancelExtract = null;
    }
  }

  resolveContentRoot(ctx) {
    const prefix = ctx.manifest.baseArchive.rootPrefix;
    if (!prefix) return ctx.stagingPath;

    const candidate = assertInside(
      ctx.stagingPath,
      path.join(ctx.stagingPath, prefix),
      "Корневая папка архива"
    );

    if (!fs.existsSync(candidate))
      throw new DeliveryError(
        "REQUIRED_FILE_MISSING",
        `После распаковки нет корневой папки "${prefix}"`
      );

    return candidate;
  }

  checkRequiredFiles(manifest, contentRoot) {
    const missing = [];

    for (const relative of manifest.filesManifest.requiredFiles) {
      const target = assertInside(
        contentRoot,
        path.join(contentRoot, relative.replace(/\//g, path.sep)),
        `Обязательный файл "${relative}"`
      );

      if (!fs.existsSync(target)) missing.push(relative);
    }

    if (missing.length)
      throw new DeliveryError(
        "REQUIRED_FILE_MISSING",
        `После распаковки не хватает файлов: ${missing.join(", ")}`,
        { missing }
      );
  }

  // Как установщик лаунчера (windows-install/install.js): не скопировался хоть
  // один файл игрока — прерываемся до подмены папок, пока прежняя сборка цела.
  preserveUserData(ctx, contentRoot) {
    if (!fs.existsSync(ctx.installPath)) return;

    const preserve = ctx.manifest.preservePaths || [];
    try {
      this.preserveDeclaredPaths(ctx, contentRoot, preserve);
      this.preserveMatchedFiles(ctx, contentRoot, preserve);
    } catch (error) {
      throw preserveError(error);
    }
  }

  // Сколько места займут копии файлов игрока в новой сборке.
  userDataBytes(ctx) {
    if (!fs.existsSync(ctx.installPath)) return 0;

    const preserve = ctx.manifest.preservePaths || [];
    return preserve
      .concat(this.matchedUserFiles(ctx, preserve))
      .reduce((total, rel) => total + treeSize(path.join(ctx.installPath, rel.replace(/\//g, path.sep))), 0);
  }

  preserveDeclaredPaths(ctx, contentRoot, preserve) {
    for (const relative of preserve) {
      const source = assertInside(
        ctx.installPath,
        path.join(ctx.installPath, relative.replace(/\//g, path.sep)),
        `Сохраняемый путь "${relative}"`
      );

      if (!fs.existsSync(source)) continue;

      const target = assertInside(
        contentRoot,
        path.join(contentRoot, relative.replace(/\//g, path.sep)),
        `Сохраняемый путь "${relative}"`
      );

      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.cpSync(source, target, { recursive: true, force: true });
    }
  }

  // files.json версии, что стоит сейчас: по нему видно, какие cfg игрок
  // поменял (user-configs.js). null — версия неизвестна.
  previousFilesManifest(ctx) {
    try {
      const installed = readJsonSafe(path.join(ctx.installPath, INSTALLED_FILE));
      if (!installed || !installed.version) return null;
      return require("./manifests").filesManifestOf(ctx.gameId, installed.version);
    } catch (error) {
      return null;
    }
  }

  preserveMatchedFiles(ctx, contentRoot, preserve) {
    const files = this.matchedUserFiles(ctx, preserve);

    for (const rel of files) {
      const target = assertInside(
        contentRoot,
        path.join(contentRoot, rel.replace(/\//g, path.sep)),
        `Пользовательский файл "${rel}"`
      );
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.copyFileSync(path.join(ctx.installPath, rel.replace(/\//g, path.sep)), target);
    }

    if (files.length)
      this.logger.info(`[game-delivery] ${ctx.gameId}: перенесено пользовательских файлов ${files.length}`);
  }

  // Файлы игрока вне preservePaths: сейвы, демки, забеги, изменённые cfg.
  matchedUserFiles(ctx, preserve) {
    const userData = createMatcher(mutablePaths.USER_DATA);
    const previous = this.previousFilesManifest(ctx);
    const matches = (rel) => userData(rel) || userConfigs.isUserEdited(ctx.installPath, rel, previous);
    const declared = createMatcher(preserve);

    const skipDir = createMatcher(preserve.concat(mutablePaths.LIBRARY_INTERNALS, mutablePaths.EPISODE_DIRS));
    const files = [];

    const walk = (dir, relative) => {
      let entries;
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch (e) {
        return;
      }

      for (const entry of entries) {
        const rel = relative ? `${relative}/${entry.name}` : entry.name;

        if (entry.isSymbolicLink()) continue;

        if (entry.isDirectory()) {
          if (!skipDir(rel)) walk(path.join(dir, entry.name), rel);
          continue;
        }

        if (matches(rel) && !declared(rel)) files.push(rel);
      }
    };

    walk(ctx.installPath, "");
    return files;
  }

  swapIntoLibrary(ctx, contentRoot) {
    fs.mkdirSync(this.paths.games, { recursive: true });

    const retired = path.join(this.paths.staging, `${path.basename(ctx.stagingPath)}-previous`);
    let retiredUsed = false;

    if (fs.existsSync(ctx.installPath)) {
      this.parkEpisodes(ctx.gameId, ctx.installPath);
      try {
        fs.renameSync(ctx.installPath, retired);
      } catch (error) {
        this.unparkEpisodes(ctx.gameId, ctx.installPath);
        throw error;
      }
      retiredUsed = true;
    }

    try {
      fs.renameSync(contentRoot, ctx.installPath);
    } catch (error) {
      if (retiredUsed && !fs.existsSync(ctx.installPath)) fs.renameSync(retired, ctx.installPath);
      this.unparkEpisodes(ctx.gameId, ctx.installPath);
      throw new DeliveryError("INSTALL_FAILED", `Не удалось перенести сборку: ${error.message}`);
    }

    this.unparkEpisodes(ctx.gameId, ctx.installPath);
    if (retiredUsed) removeWithinZone(this.paths.staging, retired);
    if (fs.existsSync(ctx.stagingPath)) removeWithinZone(this.paths.staging, ctx.stagingPath);
  }

  writeInstalled(ctx, archiveSha256, extra) {
    const installed = Object.assign(
      {
        schemaVersion: INSTALLED_SCHEMA_VERSION,
        gameId: ctx.gameId,
        version: ctx.manifest.version,
        installedAt: nowIso(),
        publishedAt: ctx.publishedAt || null,
        manifestSha256: ctx.manifestSha256 || null,
        installPath: ctx.installPath,
        archiveSha256,
      },
      extra || {},
      { state: "ready" }
    );

    writeJsonAtomic(path.join(ctx.installPath, INSTALLED_FILE), installed);

    return installed;
  }

  pendingFullUpdate(gameId) {
    const operation = this.readOperation(gameId);
    if (!operation || operation.kind !== "full-update") return null;

    return {
      gameId: operation.gameId,
      kind: operation.kind,
      phase: operation.phase,
      fromVersion: operation.fromVersion,
      toVersion: operation.toVersion,
      stagingPath: operation.stagingPath || null,
      rollbackPath: operation.rollbackPath || null,
      retiredPath: operation.retiredPath || null,
      archivePath: operation.archivePath || null,
      startedAt: operation.startedAt,
      updatedAt: operation.updatedAt,
    };
  }

  recoverFullUpdate(gameId) {
    const operation = this.readOperation(gameId);
    if (!operation || operation.kind !== "full-update") return { recovered: false };

    const installedFile = path.join(operation.installPath, INSTALLED_FILE);
    const phase = operation.phase;

    this.logger.warn(
      `[game-delivery] найдена незавершённая транзакция обновления ${operation.fromVersion} -> ` +
        `${operation.toVersion} на фазе ${phase}, восстанавливаю`
    );

    if (phase === "commit-complete" || phase === "installed-write") {
      const data = readJsonSafe(installedFile);

      if (data && data.version === operation.toVersion && data.state === "ready") {
        this.cleanupRecoveryZones(operation);
        this.clearOperation(gameId);

        this.logger.info(
          `[game-delivery] версия ${operation.toVersion} уже установлена — доубираю за собой`
        );

        return { recovered: true, direction: "forward", phase, version: operation.toVersion };
      }
    }

    if (UPDATE_SWAP_PHASES.indexOf(phase) === -1) {
      this.cleanupRecoveryZones(operation);
      this.clearOperation(gameId);
      return { recovered: true, direction: "back", phase, restoredVersion: operation.fromVersion };
    }

    const outcome = this.restoreRetired(operation);
    this.cleanupRecoveryZones(operation);
    this.clearOperation(gameId);

    if (outcome && !outcome.restored)
      return { recovered: false, direction: "kept", phase, needsVerify: true };

    return { recovered: true, direction: "back", phase, restoredVersion: operation.fromVersion };
  }

  restoreRetired(operation) {
    const installedFile = path.join(operation.installPath, INSTALLED_FILE);
    const retired = operation.retiredPath;
    const retiredExists = Boolean(retired) && fs.existsSync(retired);

    if (fs.existsSync(operation.installPath)) {
      const state = readJsonState(installedFile);
      const data = state.data;
      const oldInPlace = data && data.version === operation.fromVersion && data.state === "ready";

      if (oldInPlace) return { restored: true, action: "kept-old" };

      if (!retiredExists) {
        this.logger.error(
          `[game-delivery] ${operation.gameId}: на месте игры дерево, которое не признано ` +
            `версией ${operation.fromVersion}` +
            (state.found && !state.valid ? " (installed.json не читается)" : "") +
            ", а копии в rollback нет — оставляю как есть. " +
            "Состояние сборки нужно проверить через «Проверить файлы»"
        );

        return { restored: false, action: "kept-unknown", corrupt: state.found && !state.valid };
      }

      this.discardPlacedTree(
        operation.installPath,
        operation.stagingPath || `${operation.gameId}-recover-${Date.now().toString(36)}`
      );
    }

    if (!fs.existsSync(operation.installPath) && retiredExists)
      fs.renameSync(retired, operation.installPath);

    if (!fs.existsSync(path.join(operation.installPath, INSTALLED_FILE)))
      this.logger.error(
        `[game-delivery] после отката ${operation.gameId} нет installed.json — ` +
          "состояние сборки нужно проверить через «Проверить файлы»"
      );

    return { restored: true, action: "restored" };
  }

  cleanupRecoveryZones(operation) {
    for (const [zone, target] of [
      [this.paths.staging, operation.stagingPath],
      [this.paths.rollback, operation.rollbackPath],
    ]) {
      try {
        if (target && fs.existsSync(target)) removeWithinZone(zone, target);
      } catch (e) {
        this.logger.warn(`[game-delivery] не удалось убрать ${target}: ${e.message}`);
      }
    }
  }

  recordFailure(ctx, operation, error) {
    const logPath = path.join(this.paths.state, `${ctx.gameId}.last-failure.json`);

    try {
      fs.mkdirSync(this.paths.state, { recursive: true });
      fs.writeFileSync(
        logPath,
        JSON.stringify(
          {
            gameId: ctx.gameId,
            phase: operation.phase,
            failedAt: nowIso(),
            error: error.toPublic(),
            stagingPath: ctx.stagingPath,
            archivePath: ctx.archivePath,
          },
          null,
          2
        ),
        "utf8"
      );
    } catch (e) {
      this.logger.warn(`[game-delivery] не удалось записать диагностику: ${e.message}`);
    }

    this.logger.error(
      `[game-delivery] установка ${ctx.gameId} прервана на фазе ${operation.phase}: ${error.code} ${error.message}`
    );
  }
}

module.exports = InstallService;
module.exports.INSTALLED_SCHEMA_VERSION = INSTALLED_SCHEMA_VERSION;
module.exports.INSTALLED_FILE = INSTALLED_FILE;
module.exports.UPDATE_SWAP_PHASES = UPDATE_SWAP_PHASES;
module.exports.UPDATE_FAULTS = UPDATE_FAULTS;
module.exports.ORPHAN_STAGING_MIN_AGE_MS = ORPHAN_STAGING_MIN_AGE_MS;
