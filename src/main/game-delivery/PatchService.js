"use strict";

const fs = require("fs");
const fsp = require("fs").promises;
const path = require("path");
const { EventEmitter } = require("events");

const filesManifestLib = require("./files-manifest");
const patchFormat = require("./patch-format");
const fileOps = require("./file-ops");
const { writeJsonAtomic, readJsonSafe } = require("./atomic");
const { DeliveryError, toDeliveryError } = require("./errors");
const { assertInside, removeWithinZone, gameInstallPath } = require("./paths");
const buildMetadataTrust = require("./build-metadata-trust");
const userConfigs = require("./user-configs");

const JOURNAL_VERSION = 1;
const INSTALLED_FILE = "installed.json";
const INSTALLED_BACKUP = "installed.json.previous";
const SPACE_MARGIN = 256 * 1024 * 1024;

const COMMIT_PHASES = ["commit-started", "installed-written", "commit-complete"];

const FAULTS = [
  "before-installed-write",
  "after-installed-write",
  "before-rollback-cleanup",
];

const YIELD_EVERY = 16;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function nowIso() {
  return new Date().toISOString();
}

const shortHash = (value) => String(value || "").slice(0, 16) + "…";

function patchedFileProblem(file, stats, digest) {
  if (!stats || !stats.isFile()) return "нет на диске";
  if (stats.size !== file.size) return `размер ${stats.size}, ожидался ${file.size}`;
  if (digest !== file.sha256)
    return `SHA-256 ${shortHash(digest)}, ожидался ${shortHash(file.sha256)}`;
  return null;
}

const { freeSpaceBytes } = require("./free-space");

class PatchService extends EventEmitter {
  constructor(options) {
    const opts = options || {};
    super();

    this.paths = opts.paths;
    this.archiveService = opts.archiveService;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.spaceMargin = opts.spaceMargin == null ? SPACE_MARGIN : opts.spaceMargin;
    this.launcherVersion = opts.launcherVersion || null;
    // files.json прежней версии: по нему видно, какие cfg игрок поменял.
    this.filesManifestOf = opts.filesManifestOf || ((gameId, version) => require("./manifests").filesManifestOf(gameId, version));

    this.faultInjector = opts.faultInjector || null;
    this.busy = false;
  }

  fault(name, journal) {
    if (this.faultInjector) this.faultInjector(name, journal);
  }

  journalPath(gameId) {
    return path.join(this.paths.state, `${gameId}.patch.json`);
  }

  writeJournal(journal) {
    fs.mkdirSync(this.paths.state, { recursive: true });
    journal.updatedAt = nowIso();
    writeJsonAtomic(
      this.journalPath(journal.gameId),
      Object.assign({ version: JOURNAL_VERSION }, journal)
    );
  }

  readJournal(gameId) {
    const file = this.journalPath(gameId);
    if (!fs.existsSync(file)) return null;

    const data = readJsonSafe(file);

    if (!data) return { corrupt: true, gameId };

    return data.version === JOURNAL_VERSION ? data : { corrupt: true, gameId };
  }

  quarantineJournal(gameId) {
    const file = this.journalPath(gameId);
    const target = `${file}.corrupt-${Date.now().toString(36)}`;

    try {
      fs.renameSync(file, target);
    } catch (error) {
      this.logger.warn(`[game-delivery] повреждённый журнал не убран: ${error.message}`);
      return null;
    }

    this.logger.error(
      `[game-delivery] журнал патча ${gameId} повреждён и перемещён в ${target}. ` +
        "Состояние сборки нужно проверить через «Проверить файлы»."
    );

    return target;
  }

  clearJournal(gameId) {
    try {
      const file = this.journalPath(gameId);
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch (e) {
      this.logger.warn(`[game-delivery] журнал патча не удалён: ${e.message}`);
    }
  }

  setPhase(journal, phase) {
    journal.phase = phase;
    this.writeJournal(journal);
    this.emit("state", { gameId: journal.gameId, state: "applying-update", phase });
  }

  findPending(gameId) {
    const journal = this.readJournal(gameId);
    if (!journal) return null;

    if (journal.corrupt) return { gameId, corrupt: true, phase: "unknown", needsRollback: false };

    return {
      gameId: journal.gameId,
      fromVersion: journal.fromVersion,
      toVersion: journal.toVersion,
      phase: journal.phase,
      startedAt: journal.startedAt,
      updatedAt: journal.updatedAt,
      needsRollback: ["replace", "delete", "verify"].indexOf(journal.phase) !== -1,
      rollsForward: COMMIT_PHASES.indexOf(journal.phase) !== -1,
      rollbackPath: journal.rollbackPath || null,
    };
  }

  recover(gameId) {
    const journal = this.readJournal(gameId);
    if (!journal) return { recovered: false };

    if (journal.corrupt) {
      const quarantined = this.quarantineJournal(gameId);
      return { recovered: false, corrupt: true, quarantined, verifyRecommended: true };
    }

    if (COMMIT_PHASES.indexOf(journal.phase) !== -1) return this.recoverCommit(journal);

    this.logger.warn(
      `[game-delivery] найдена незавершённая транзакция патча ${journal.fromVersion} -> ` +
        `${journal.toVersion} на фазе ${journal.phase}, выполняю откат`
    );

    this.rollback(journal);
    this.cleanupTransaction(journal);
    this.clearJournal(gameId);

    return {
      recovered: true,
      direction: "back",
      phase: journal.phase,
      restoredVersion: journal.fromVersion,
    };
  }

  recoverCommit(journal) {
    const gameId = journal.gameId;
    const installedFile = path.join(journal.installPath, INSTALLED_FILE);
    const installed = readJsonSafe(installedFile);

    if (installed && installed.version === journal.toVersion) {
      this.logger.info(
        `[game-delivery] транзакция патча ${gameId} была прервана после записи installed.json — ` +
          `версия ${journal.toVersion} уже установлена, доубираю за собой`
      );

      this.cleanupTransaction(journal);
      this.clearJournal(gameId);

      return { recovered: true, direction: "forward", phase: journal.phase, version: journal.toVersion };
    }

    let problems = null;

    if (Array.isArray(journal.expectedFiles)) {
      try {
        problems = this.verifyResultSync(journal.installPath, {
          files: journal.expectedFiles,
          deletedFiles: journal.expectedDeleted || [],
        });
      } catch (error) {
        this.logger.error(`[game-delivery] проверка файлов патча не удалась: ${error.message}`);
        problems = null;
      }
    } else {
      this.logger.warn(
        `[game-delivery] в журнале патча ${gameId} нет списка файлов (журнал от прежней версии) — откатываюсь`
      );
    }

    if (problems && problems.length === 0) {
      this.logger.warn(
        `[game-delivery] транзакция патча ${gameId} прервана перед записью installed.json, ` +
          "но файлы новой версии на месте — дописываю installed.json"
      );

      this.writeInstalledAtomic(journal.installPath, {
        version: journal.toVersion,
        migratedFrom: journal.fromVersion,
        filesManifestSha256: journal.resultingFilesManifestSha256 || null,
      });

      this.cleanupTransaction(journal);
      this.clearJournal(gameId);

      return { recovered: true, direction: "forward", phase: journal.phase, version: journal.toVersion };
    }

    this.logger.error(
      `[game-delivery] транзакция патча ${gameId} прервана на фазе ${journal.phase}, ` +
        (problems && problems.length
          ? `не совпало файлов: ${problems.length} (${problems
              .slice(0, 3)
              .map((p) => p.path)
              .join(", ")}) — откатываюсь к прежней версии`
          : "откатываюсь к прежней версии")
    );

    this.rollback(journal);
    this.restoreInstalledBackup(journal);
    this.cleanupTransaction(journal);
    this.clearJournal(gameId);

    return {
      recovered: true,
      direction: "back",
      phase: journal.phase,
      restoredVersion: journal.fromVersion,
    };
  }

  async apply(request) {
    if (this.busy)
      throw new DeliveryError("ALREADY_RUNNING", "Обновление уже выполняется");

    const gameManifest = request.gameManifest;
    const patchManifest = request.patchManifest;
    const gameId = gameManifest.gameId;
    const installPath = gameInstallPath(this.paths, gameId);

    patchFormat.validatePatchManifest(patchManifest, {
      expectedFrom: request.installedVersion,
    });

    if (patchManifest.gameId !== gameId)
      throw new DeliveryError("MANIFEST_INVALID", "Патч собран для другой игры");

    const id = `${gameId}-${patchManifest.fromVersion}-to-${patchManifest.toVersion}-${Date.now().toString(36)}`;
    const stagingPath = assertInside(this.paths.staging, path.join(this.paths.staging, id), "staging патча");
    const rollbackPath = assertInside(this.paths.rollback, path.join(this.paths.rollback, id), "rollback патча");

    const journal = {
      gameId,
      fromVersion: patchManifest.fromVersion,
      toVersion: patchManifest.toVersion,
      phase: "preflight",
      installPath,
      stagingPath,
      rollbackPath,
      archivePath: request.archivePath,
      replaced: [],
      added: [],
      deleted: [],
      startedAt: nowIso(),
    };

    this.busy = true;
    this.writeJournal(journal);

    try {
      const result = await this.runTransaction({
        journal,
        gameManifest,
        patchManifest,
        installPath,
        archivePath: request.archivePath,
        filesManifest: request.filesManifest || null,
      });

      this.cleanupTransaction(journal);
      this.clearJournal(gameId);

      return result;
    } catch (error) {
      const delivery = toDeliveryError(error, "PATCH_FAILED");

      this.logger.error(
        `[game-delivery] патч ${journal.fromVersion} -> ${journal.toVersion} прерван ` +
          `на фазе ${journal.phase}: ${delivery.code} ${delivery.message}`
      );

      try {
        this.rollback(journal);
        if (COMMIT_PHASES.indexOf(journal.phase) !== -1) this.restoreInstalledBackup(journal);
      } catch (rollbackError) {
        this.logger.error(`[game-delivery] откат не удался: ${rollbackError.message}`);
        this.emit("state", { gameId, state: "error", error: delivery.toPublic() });

        throw new DeliveryError(
          "ROLLBACK_FAILED",
          `Патч не применён и откат не удался: ${rollbackError.message}. ` +
            `Файлы прежней версии сохранены в ${journal.rollbackPath}`,
          { rollbackPath: journal.rollbackPath, original: delivery.toPublic() }
        );
      }

      this.cleanupTransaction(journal);
      this.clearJournal(gameId);
      this.emit("state", { gameId, state: "error", error: delivery.toPublic() });

      throw delivery;
    } finally {
      this.busy = false;
    }
  }

  async runTransaction(context) {
    const { journal, patchManifest, installPath, archivePath } = context;

    this.setPhase(journal, "preflight");
    this.assertInstalled(installPath, patchManifest.fromVersion);
    this.assertGameNotRunning(installPath);
    this.assertSpace(patchManifest);

    this.setPhase(journal, "verify-archive");
    this.verifyArchiveFile(archivePath, patchManifest);

    this.setPhase(journal, "inspect");
    const listing = await this.archiveService.list(archivePath);
    patchFormat.validatePayloadEntries(patchManifest, listing.entries);

    this.setPhase(journal, "extract");
    fs.mkdirSync(journal.stagingPath, { recursive: true });
    await this.extract(archivePath, journal.stagingPath);

    this.setPhase(journal, "verify-payload");
    const payloadRoot = assertInside(
      journal.stagingPath,
      path.join(journal.stagingPath, patchFormat.PAYLOAD_ROOT),
      "payload патча"
    );
    await this.verifyPayload(payloadRoot, patchManifest);

    const applied = this.withoutUserConfigs(journal.gameId, installPath, patchManifest);

    this.setPhase(journal, "backup");
    fs.mkdirSync(journal.rollbackPath, { recursive: true });
    this.backupAffected(journal, installPath, applied);

    this.setPhase(journal, "replace");
    this.replaceFiles(journal, payloadRoot, installPath, applied);

    this.setPhase(journal, "delete");
    this.deleteFiles(journal, installPath, applied);

    this.setPhase(journal, "verify");
    const verification = await this.verifyResult(context, installPath, applied);

    const installed = this.commit(journal, installPath, applied, verification, context);

    this.emit("state", { gameId: journal.gameId, state: "ready", installed });

    return { status: "applied", installed, verification };
  }

  // cfg, изменённые игроком, патч не заменяет и не удаляет (user-configs.js).
  withoutUserConfigs(gameId, installPath, patchManifest) {
    let previous = null;
    try {
      previous = this.filesManifestOf(gameId, patchManifest.fromVersion);
    } catch (error) {
      previous = null;
    }
    const kept = userConfigs.keptByPatch(installPath, patchManifest, previous);
    if (!kept.size) return patchManifest;
    this.logger.info(`[game-delivery] ${gameId}: cfg игрока оставлены как есть: ${Array.from(kept).join(", ")}`);
    return Object.assign({}, patchManifest, {
      files: patchManifest.files.filter((file) => !kept.has(file.path)),
      deletedFiles: patchManifest.deletedFiles.filter((relative) => !kept.has(relative)),
    });
  }

  commit(journal, installPath, patchManifest, verification, context) {
    const installedFile = path.join(installPath, INSTALLED_FILE);

    journal.resultingFilesManifestSha256 = patchManifest.resultingFilesManifestSha256;
    journal.installedBackup = path.join(journal.rollbackPath, INSTALLED_BACKUP);

    journal.expectedFiles = patchManifest.files.map((file) => ({
      path: file.path,
      size: file.size,
      sha256: file.sha256,
    }));
    journal.expectedDeleted = patchManifest.deletedFiles.slice();

    this.setPhase(journal, "commit-started");

    if (fs.existsSync(installedFile))
      fileOps.copyFileSync(installedFile, journal.installedBackup, INSTALLED_FILE);

    this.fault("before-installed-write", journal);

    const changes = {
      version: patchManifest.toVersion,
      migratedFrom: patchManifest.fromVersion,
      filesManifestSha256: verification.filesManifestSha256,
      totalFiles: verification.totalFiles,
      totalSize: verification.totalSize,
    };

    const snapshot = this.buildMetadataSnapshot(installPath, patchManifest, context);
    if (snapshot) {
      changes.buildMetadata = snapshot.buildMetadata;
      changes.buildMetadataSha256 = snapshot.buildMetadataSha256;
      changes.buildMetadataVerifiedAt = snapshot.buildMetadataVerifiedAt;
    }

    const installed = this.writeInstalledAtomic(installPath, changes);

    this.setPhase(journal, "installed-written");
    this.fault("after-installed-write", journal);

    this.setPhase(journal, "commit-complete");
    this.fault("before-rollback-cleanup", journal);

    return installed;
  }

  assertInstalled(installPath, expectedVersion) {
    const file = path.join(installPath, INSTALLED_FILE);

    if (!fs.existsSync(file))
      throw new DeliveryError("NOT_INSTALLED", "Игра не установлена — патч применять не к чему");

    const installed = JSON.parse(fs.readFileSync(file, "utf8"));

    if (installed.state !== "ready")
      throw new DeliveryError("NOT_INSTALLED", `Установка не завершена (state: ${installed.state})`);

    if (installed.version !== expectedVersion)
      throw new DeliveryError(
        "VERSION_MISMATCH",
        `Патч рассчитан на версию ${expectedVersion}, установлена ${installed.version}`,
        { expected: expectedVersion, installed: installed.version }
      );

    return installed;
  }

  assertGameNotRunning(installPath) {
    const check = fileOps.isGameRunning(installPath, ["hl2.exe"]);

    if (check.running)
      throw new DeliveryError(
        "GAME_RUNNING",
        `Игра запущена (${check.path}). Закройте её и повторите обновление.`,
        { path: check.path }
      );
  }

  assertSpace(patchManifest) {
    const free = freeSpaceBytes(this.paths.staging);
    if (free == null) return;

    const required = patchManifest.archive.size * 2 + this.spaceMargin;

    if (free < required)
      throw new DeliveryError("NO_SPACE", `Недостаточно места: нужно ${required}, свободно ${free}`, {
        requiredBytes: required,
        freeBytes: free,
      });
  }

  verifyArchiveFile(archivePath, patchManifest) {
    let stats;
    try {
      stats = fs.statSync(archivePath);
    } catch (e) {
      throw new DeliveryError("SIZE_MISMATCH", `Архив патча не найден: ${archivePath}`);
    }

    if (stats.size !== patchManifest.archive.size)
      throw new DeliveryError(
        "SIZE_MISMATCH",
        `Размер патча ${stats.size} не совпадает с манифестом ${patchManifest.archive.size}`
      );

    const digest = filesManifestLib.hashFileSync(archivePath);

    if (digest !== patchManifest.archive.sha256)
      throw new DeliveryError("CHECKSUM_MISMATCH", "SHA-256 архива патча не совпадает", {
        expected: patchManifest.archive.sha256,
        actual: digest,
      });
  }

  async extract(archivePath, destination) {
    const job = this.archiveService.extract(archivePath, destination, {
      onProgress: (fraction) =>
        this.emit("state", {
          state: "applying-update",
          phase: "extract",
          percent: Math.round(fraction * 100),
        }),
    });

    await job.promise;
  }

  async verifyPayload(payloadRoot, patchManifest) {
    for (const file of patchManifest.files) {
      const source = fileOps.resolveInside(payloadRoot, file.path, `Файл патча "${file.path}"`);

      if (!fs.existsSync(source))
        throw new DeliveryError("REQUIRED_FILE_MISSING", `В payload нет файла ${file.path}`);

      const size = fs.statSync(source).size;
      if (size !== file.size)
        throw new DeliveryError(
          "SIZE_MISMATCH",
          `Размер ${file.path} в патче ${size}, ожидался ${file.size}`
        );

      const digest = await fileOps.hashFile(source);
      if (digest !== file.sha256)
        throw new DeliveryError("CHECKSUM_MISMATCH", `SHA-256 не совпал для ${file.path}`, {
          path: file.path,
        });
    }
  }

  backupAffected(journal, installPath, patchManifest) {
    for (const file of patchManifest.files) {
      const target = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

      if (fs.existsSync(target)) {
        const backup = fileOps.resolveInside(journal.rollbackPath, file.path, "Резервная копия");
        fileOps.copyFileSync(target, backup, file.path);
        journal.replaced.push(file.path);
      } else {
        journal.added.push(file.path);
      }
    }

    for (const relativePath of patchManifest.deletedFiles) {
      const target = fileOps.resolveInside(installPath, relativePath, `Файл "${relativePath}"`);
      if (!fs.existsSync(target)) continue;

      const backup = fileOps.resolveInside(journal.rollbackPath, relativePath, "Резервная копия");
      fileOps.copyFileSync(target, backup, relativePath);
      journal.deleted.push(relativePath);
    }

    this.writeJournal(journal);
  }

  replaceFiles(journal, payloadRoot, installPath, patchManifest) {
    for (const file of patchManifest.files) {
      const source = fileOps.resolveInside(payloadRoot, file.path, `Файл патча "${file.path}"`);
      const target = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

      fileOps.copyFileSync(source, target, file.path);
    }
  }

  deleteFiles(journal, installPath, patchManifest) {
    for (const relativePath of patchManifest.deletedFiles) {
      const target = fileOps.resolveInside(installPath, relativePath, `Файл "${relativePath}"`);
      if (!fs.existsSync(target)) continue;

      fileOps.removeFileSync(target, relativePath);
      fileOps.pruneEmptyDirs(installPath, path.dirname(target));
    }
  }

  async verifyResult(context, installPath, patchManifest) {
    const problems = [];

    for (let index = 0; index < patchManifest.files.length; index++) {
      const file = patchManifest.files[index];
      const full = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

      let stats = null;
      try {
        stats = await fsp.stat(full);
      } catch (e) {
        stats = null;
      }

      const digest =
        stats && stats.isFile() && stats.size === file.size ? await fileOps.hashFile(full) : null;

      const problem = patchedFileProblem(file, stats, digest);
      if (problem) problems.push({ path: file.path, reason: problem });

      if (index % YIELD_EVERY === 0) await delay(0);
    }

    for (const relativePath of patchManifest.deletedFiles) {
      const full = fileOps.resolveInside(installPath, relativePath, `Файл "${relativePath}"`);
      if (fs.existsSync(full))
        problems.push({ path: relativePath, reason: "должен был исчезнуть, но остался" });
    }

    if (problems.length) {
      const shown = problems.slice(0, 10);
      throw new DeliveryError(
        "VERIFY_FAILED",
        `Патч лёг не полностью, не совпало файлов: ${problems.length}. ` +
          shown.map((p) => `${p.path} — ${p.reason}`).join("; ") +
          (problems.length > shown.length ? ` (и ещё ${problems.length - shown.length})` : ""),
        { problems: shown, total: problems.length }
      );
    }

    return {
      filesManifestSha256: patchManifest.resultingFilesManifestSha256,
      totalFiles: patchManifest.resultingTotalFiles,
      totalSize: patchManifest.resultingTotalSize,
      changedFiles: patchManifest.files.length,
      deletedFiles: patchManifest.deletedFiles.length,
    };
  }

  buildMetadataSnapshot(installPath, patchManifest, context) {
    const gameManifest = (context && context.gameManifest) || null;
    const descriptor = gameManifest && gameManifest.buildMetadata;
    if (!descriptor) return null;

    const entry = patchManifest.files.find((file) => file.path === buildMetadataTrust.RELATIVE_PATH);
    if (!entry) return null;

    try {
      const verified = buildMetadataTrust.verifyForInstall({
        installRoot: installPath,
        descriptor,
        manifest: gameManifest,
        launcherVersion: this.launcherVersion,
      });

      return buildMetadataTrust.snapshotFromVerified(verified);
    } catch (error) {
      this.logger.warn(
        `[game-delivery] сведения о сборке после патча не обновлены (${error.code || "?"}): ${error.message}`
      );
      return null;
    }
  }

  verifyResultSync(installPath, patchManifest) {
    const problems = [];

    for (const file of patchManifest.files) {
      const full = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

      let stats = null;
      try {
        stats = fs.statSync(full);
      } catch (e) {
        stats = null;
      }

      const digest =
        stats && stats.isFile() && stats.size === file.size
          ? filesManifestLib.hashFileSync(full)
          : null;

      const problem = patchedFileProblem(file, stats, digest);
      if (problem) problems.push({ path: file.path, reason: problem });
    }

    for (const relativePath of patchManifest.deletedFiles) {
      const full = fileOps.resolveInside(installPath, relativePath, `Файл "${relativePath}"`);
      if (fs.existsSync(full))
        problems.push({ path: relativePath, reason: "должен был исчезнуть, но остался" });
    }

    return problems;
  }

  writeInstalledAtomic(installPath, changes) {
    const file = path.join(installPath, INSTALLED_FILE);
    const installed = readJsonSafe(file) || {};

    const updated = Object.assign({}, installed, changes, {
      updatedAt: nowIso(),
      state: "ready",
    });

    writeJsonAtomic(file, updated);
    return updated;
  }

  restoreInstalledBackup(journal) {
    const backup = journal.installedBackup;
    if (!backup || !fs.existsSync(backup)) return false;

    fileOps.copyFileSync(backup, path.join(journal.installPath, INSTALLED_FILE), INSTALLED_FILE);
    return true;
  }

  rollback(journal) {
    if (!journal.rollbackPath || !fs.existsSync(journal.rollbackPath)) return { restored: 0 };

    const installPath = journal.installPath;
    let restored = 0;

    for (const relativePath of (journal.replaced || []).concat(journal.deleted || [])) {
      const backup = fileOps.resolveInside(journal.rollbackPath, relativePath, "Резервная копия");
      if (!fs.existsSync(backup)) continue;

      const target = fileOps.resolveInside(installPath, relativePath, `Файл "${relativePath}"`);
      fileOps.copyFileSync(backup, target, relativePath);
      restored++;
    }

    for (const relativePath of journal.added || []) {
      const target = fileOps.resolveInside(installPath, relativePath, `Файл "${relativePath}"`);
      if (!fs.existsSync(target)) continue;

      fileOps.removeFileSync(target, relativePath);
      fileOps.pruneEmptyDirs(installPath, path.dirname(target));
    }

    this.logger.info(`[game-delivery] откат патча: восстановлено файлов — ${restored}`);
    return { restored };
  }

  cleanupTransaction(journal) {
    for (const [zone, target] of [
      [this.paths.staging, journal.stagingPath],
      [this.paths.rollback, journal.rollbackPath],
    ]) {
      try {
        if (target && fs.existsSync(target)) removeWithinZone(zone, target);
      } catch (e) {
        this.logger.warn(`[game-delivery] не удалось убрать ${target}: ${e.message}`);
      }
    }
  }
}

module.exports = PatchService;
module.exports.JOURNAL_VERSION = JOURNAL_VERSION;
module.exports.COMMIT_PHASES = COMMIT_PHASES;
module.exports.FAULTS = FAULTS;
