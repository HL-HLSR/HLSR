"use strict";

const fs = require("fs");
const path = require("path");
const { EventEmitter } = require("events");

const fileOps = require("./file-ops");
const { createMatcher } = require("./path-match");
const { matchesMutable } = require("./mutable-paths");
const { writeJsonAtomic, readJsonSafe } = require("./atomic");
const { DeliveryError, toDeliveryError } = require("./errors");
const { assertInside, assertRealInside, removeWithinZone } = require("./paths");

const MAX_FILES_PER_PASS = 2000;
const JOURNAL_VERSION = 1;

const FAULTS = ["after-replace", "before-recheck"];

function nowIso() {
  return new Date().toISOString();
}

class RepairService extends EventEmitter {
  constructor(options) {
    const opts = options || {};
    super();

    this.paths = opts.paths;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.faultInjector = opts.faultInjector || null;
    this.busy = false;
  }

  fault(name, context) {
    if (this.faultInjector) this.faultInjector(name, context);
  }

  journalPath(gameId) {
    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(String(gameId || "")))
      throw new DeliveryError("PATH_INVALID", `Недопустимый gameId: ${gameId}`);

    return path.join(this.paths.state, `${gameId}.repair.json`);
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

  clearJournal(gameId) {
    try {
      const file = this.journalPath(gameId);
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch (error) {
      this.logger.warn(`[game-delivery] журнал ремонта не удалён: ${error.message}`);
    }
  }

  setPhase(journal, phase) {
    journal.phase = phase;
    this.writeJournal(journal);
    this.emit("state", { gameId: journal.gameId, state: "repairing", phase });
  }

  findPending(gameId) {
    const journal = this.readJournal(gameId);
    if (!journal) return null;

    if (journal.corrupt) return { gameId, corrupt: true, phase: "unknown" };

    return {
      gameId: journal.gameId,
      phase: journal.phase,
      startedAt: journal.startedAt,
      updatedAt: journal.updatedAt,
      replaced: (journal.replaced || []).length,
      created: (journal.created || []).length,
      needsRollback: ["replace", "recheck"].indexOf(journal.phase) !== -1,
    };
  }

  recover(gameId) {
    const journal = this.readJournal(gameId);
    if (!journal) return { recovered: false };

    if (journal.corrupt) {
      this.clearJournal(gameId);
      return { recovered: false, corrupt: true, verifyRecommended: true };
    }

    this.logger.warn(
      `[game-delivery] найден незавершённый ремонт ${gameId} на фазе ${journal.phase}, ` +
        "возвращаю файлы в исходное состояние"
    );

    const result = this.rollback(journal);
    this.cleanupTransaction(journal);
    this.clearJournal(gameId);

    return Object.assign({ recovered: true, phase: journal.phase }, result);
  }

  // Эталонные файлы приходят только из пофайлового зеркала (repair-mirror.js).
  preparedRoot(request) {
    const root = request.preparedRoot;

    if (!root || !fs.existsSync(root))
      throw new DeliveryError(
        "REPAIR_SOURCE_MISSING",
        `Каталог подготовленных эталонных файлов не существует: ${root}`
      );

    return root;
  }

  async repair(request) {
    if (this.busy) throw new DeliveryError("ALREADY_RUNNING", "Восстановление уже выполняется");

    const installPath = request.installPath;
    const manifest = request.filesManifest;
    const preserved = createMatcher(request.preservePaths || []);

    // Стоковые cfg из папок игрока чинятся только по подписанному списку версии.
    const stock = new Set((manifest.stockConfigs || []).map((file) => file.path));
    const byPath = new Map(manifest.files.concat(manifest.stockConfigs || []).map((file) => [file.path, file]));
    const wanted = [];

    for (const item of request.damagedFiles || []) {
      const relative = typeof item === "string" ? item : item.path;

      if (!stock.has(relative) && (preserved(relative) || matchesMutable(relative)))
        throw new DeliveryError(
          "REPAIR_FAILED",
          `Файл ${relative} относится к пользовательским данным и не восстанавливается`
        );

      const entry = byPath.get(relative);
      if (!entry)
        throw new DeliveryError("REPAIR_FAILED", `Файла ${relative} нет в файловом манифесте`);

      wanted.push(entry);
    }

    if (!wanted.length) return { status: "nothing-to-repair", repaired: [], failed: [] };

    if (wanted.length > MAX_FILES_PER_PASS)
      throw new DeliveryError(
        "REPAIR_FAILED",
        `Повреждено слишком много файлов (${wanted.length}) — надёжнее переустановить сборку`,
        { damaged: wanted.length }
      );

    const prepared = this.preparedRoot(request);

    const check = fileOps.isGameRunning(installPath, ["hl2.exe"]);
    if (check.running)
      throw new DeliveryError(
        "GAME_RUNNING",
        `Игра запущена (${check.path}). Закройте её и повторите восстановление.`
      );

    const id = `repair-${request.gameId}-${Date.now().toString(36)}`;
    const rollbackPath = assertInside(this.paths.rollback, path.join(this.paths.rollback, id), "rollback ремонта");

    const journal = {
      gameId: request.gameId,
      phase: "preflight",
      installPath,
      rollbackPath,
      preparedRoot: prepared,
      replaced: [],
      created: [],
      startedAt: nowIso(),
    };

    this.busy = true;

    try {
      fs.mkdirSync(rollbackPath, { recursive: true });
      this.writeJournal(journal);

      const result = await this.runTransaction(journal, request, wanted);

      this.cleanupTransaction(journal);
      this.clearJournal(journal.gameId);

      return result;
    } catch (error) {
      const delivery = toDeliveryError(error, "REPAIR_FAILED");

      try {
        this.rollback(journal);
      } catch (rollbackError) {
        this.logger.error(`[game-delivery] откат ремонта не удался: ${rollbackError.message}`);

        this.emit("state", { gameId: journal.gameId, state: "error", error: delivery.toPublic() });

        throw new DeliveryError(
          "ROLLBACK_FAILED",
          `Ремонт не выполнен и откат не удался: ${rollbackError.message}. ` +
            `Исходные файлы сохранены в ${rollbackPath}`,
          { rollbackPath, original: delivery.toPublic() }
        );
      }

      this.cleanupTransaction(journal);
      this.clearJournal(journal.gameId);

      this.emit("state", { gameId: journal.gameId, state: "error", error: delivery.toPublic() });
      throw delivery;
    } finally {
      this.busy = false;
    }
  }

  async runTransaction(journal, request, wanted) {
    const installPath = journal.installPath;
    const extractedRoot = journal.preparedRoot;

    this.setPhase(journal, "verify-source");

    for (const file of wanted) {
      const source = fileOps.resolveInside(extractedRoot, file.path, `Эталон "${file.path}"`);
      const target = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

      // Ни одной записи, пока у всех целей путь не проверен и после разворота
      // ссылок: junction вместо папки игры увёл бы файл за её пределы.
      assertRealInside(extractedRoot, source, `Эталон "${file.path}"`);
      assertRealInside(installPath, target, `Файл "${file.path}"`);

      if (!fs.existsSync(source))
        throw new DeliveryError("REPAIR_FAILED", `В наборе эталонных файлов нет ${file.path}`);

      const size = fs.statSync(source).size;
      if (size !== file.size)
        throw new DeliveryError(
          "REPAIR_FAILED",
          `Размер эталонного файла ${file.path} не совпал с манифестом`,
          { path: file.path, expected: file.size, actual: size }
        );

      const digest = await fileOps.hashFile(source);
      if (digest !== file.sha256)
        throw new DeliveryError(
          "REPAIR_FAILED",
          `Эталонный файл ${file.path} не совпал с манифестом`,
          { path: file.path, expected: file.sha256, actual: digest }
        );
    }

    this.setPhase(journal, "replace");
    const restored = [];

    for (let index = 0; index < wanted.length; index++) {
      const file = wanted[index];
      const source = fileOps.resolveInside(extractedRoot, file.path, `Эталон "${file.path}"`);
      const target = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);

      if (fs.existsSync(target)) {
        const backup = fileOps.resolveInside(journal.rollbackPath, file.path, "Резервная копия");
        fileOps.copyFileSync(target, backup, file.path);
        journal.replaced.push(file.path);
      } else {
        journal.created.push(file.path);
      }

      this.writeJournal(journal);

      fileOps.copyFileSync(source, target, file.path);
      restored.push(file.path);

      this.fault("after-replace", { index, path: file.path, journal });

      this.emit("progress", {
        gameId: request.gameId,
        checked: index + 1,
        total: wanted.length,
        percent: ((index + 1) / wanted.length) * 100,
      });
    }

    this.setPhase(journal, "recheck");
    this.fault("before-recheck", { journal });

    const failed = [];

    for (const file of wanted) {
      const target = fileOps.resolveInside(installPath, file.path, `Файл "${file.path}"`);
      const digest = await fileOps.hashFile(target);
      if (digest !== file.sha256) failed.push(file.path);
    }

    if (failed.length)
      throw new DeliveryError(
        "REPAIR_FAILED",
        `После замены не совпали файлы: ${failed.slice(0, 5).join(", ")}`,
        { failed }
      );

    this.logger.info(
      `[game-delivery] восстановлено файлов: ${restored.length} (${request.gameId}), источник — зеркало`
    );

    this.emit("state", {
      gameId: request.gameId,
      state: "repair-complete",
      repaired: restored.length,
    });

    return {
      status: "repaired",
      repaired: restored,
      failed: [],
    };
  }

  rollback(journal) {
    const installPath = journal.installPath;
    let restored = 0;
    let removed = 0;

    for (const relative of journal.replaced || []) {
      const backup = fileOps.resolveInside(journal.rollbackPath, relative, "Резервная копия");
      if (!fs.existsSync(backup)) continue;

      const target = fileOps.resolveInside(installPath, relative, `Файл "${relative}"`);
      fileOps.copyFileSync(backup, target, relative);
      restored++;
    }

    for (const relative of journal.created || []) {
      const target = fileOps.resolveInside(installPath, relative, `Файл "${relative}"`);
      if (!fs.existsSync(target)) continue;

      fileOps.removeFileSync(target, relative);
      fileOps.pruneEmptyDirs(installPath, path.dirname(target));
      removed++;
    }

    this.logger.info(
      `[game-delivery] откат ремонта: возвращено ${restored}, удалено созданных ${removed}`
    );

    return { restored, removed };
  }

  cleanupTransaction(journal) {
    for (const [zone, target] of [
      [this.paths.staging, journal.stagingPath],
      [this.paths.rollback, journal.rollbackPath],
    ]) {
      try {
        if (target && fs.existsSync(target)) removeWithinZone(zone, target);
      } catch (error) {
        this.logger.warn(`[game-delivery] не удалось убрать ${target}: ${error.message}`);
      }
    }
  }
}

module.exports = RepairService;
module.exports.JOURNAL_VERSION = JOURNAL_VERSION;
module.exports.FAULTS = FAULTS;
