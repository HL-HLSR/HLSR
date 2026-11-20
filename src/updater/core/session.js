"use strict";

const { EventEmitter } = require("events");

const path = require("path");

const channel = require("../shared/launcher-update/launcher-channel");
const chunks = require("../shared/launcher-update/chunks");
const { CODES, UpdaterError, toUpdaterError } = require("./errors");
const downloadLib = require("./download");
const processWait = require("./process-wait");
const installLib = require("./install");
const receiptLib = require("./receipt");

const STAGES = {
  DOWNLOAD: "download",
  VERIFY: "verify",
  WAIT_EXIT: "wait-exit",
  INSTALL: "install",
  RELAUNCH: "relaunch",
  DONE: "done",
  ERROR: "error",
  CANCELLED: "cancelled",
};

const CANCELLABLE_STAGES = [STAGES.DOWNLOAD];

class UpdateSession extends EventEmitter {
  constructor(options) {
    super();
    const opts = options || {};

    this.plan = opts.plan;
    this.workDir = opts.workDir;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };

    const deps = opts.deps || {};
    this.download = deps.download || downloadLib.download;
    this.verifyInstaller = deps.verifyInstaller || channel.verifyInstallerStream;
    this.waitForExit = deps.waitForExit || processWait.waitForExit;
    this.runInstaller = deps.runInstaller || installLib.runInstaller;
    this.verifyInstalled = deps.verifyInstalled || installLib.verifyInstalled;
    this.relaunch = deps.relaunch || installLib.relaunch;
    this.writeReceipt = deps.writeReceipt || receiptLib.writePendingReceipt;
    this.fetchChunk = deps.fetchChunk;
    this.assemble = deps.assemble || chunks.assemble;
    this.fetchManifest = deps.fetchManifest || chunks.fetchManifest;

    this.stage = null;
    this.cancelRequested = false;
    this.installerPath = null;
    this.finished = false;
  }

  setStage(stage, extra) {
    this.stage = stage;
    this.emit("stage", Object.assign({ stage, cancellable: this.isCancellable() }, extra || {}));
  }

  isCancellable() {
    return CANCELLABLE_STAGES.indexOf(this.stage) !== -1 && !this.finished;
  }

  cancel() {
    if (!this.isCancellable()) return false;
    this.cancelRequested = true;
    if (this.abort) this.abort.abort();
    this.logger.info("[updater] запрошена отмена на стадии загрузки");
    return true;
  }

  async run() {
    try {
      const result = await this.execute();
      this.finished = true;
      return result;
    } catch (error) {
      this.finished = true;

      const updaterError = toUpdaterError(error, CODES.INSTALL_FAILED);

      if (updaterError.code === CODES.CANCELLED) {
        this.setStage(STAGES.CANCELLED);
        this.emit("cancelled");
        return { ok: false, cancelled: true };
      }

      this.logger.error(`[updater] ${updaterError.code}: ${updaterError.message}`);
      this.setStage(STAGES.ERROR, { error: updaterError.toPublic() });
      this.emit("failed", updaterError.toPublic());

      return { ok: false, error: updaterError.toPublic() };
    }
  }

  async execute() {
    const plan = this.plan;
    const delta = channel.deltaOf(plan.manifest);

    if (delta) {
      const tree = await this.prepareTree(delta);
      if (tree) {
        try {
          return await this.installFrom(path.join(tree, "HLSR.exe"), installLib.treeSetupOptions(tree, plan.installDir, plan.targetVersion));
        } catch (error) {
          if (error && error.code === CODES.CANCELLED) throw error;
          this.logger.warn(`[updater] установка собранной версии не удалась, качаю установщик: ${error.message}`);
        }
      }
    }

    this.setStage(STAGES.DOWNLOAD, { done: 0, total: plan.installer.size, bytesPerSecond: 0 });

    this.installerPath = await this.download({
      urls: plan.installer.urls,
      dir: this.workDir,
      filename: plan.installer.filename,
      size: plan.installer.size,
      isAllowedUrl: (url) => channel.isAllowedUrl(url),
      isCancelled: () => this.cancelRequested,
      onProgress: (progress) => {
        this.emit("progress", progress);
      },
    });

    if (this.cancelRequested) throw new UpdaterError(CODES.CANCELLED, "отменено");

    this.logger.info(`[updater] установщик загружен: ${plan.installer.filename}`);

    this.setStage(STAGES.VERIFY);

    const verdict = await this.verifyInstaller(this.installerPath, {
      filename: plan.installer.filename,
      size: plan.installer.size,
      sha256: plan.installer.sha256,
      sha512: plan.installer.sha512,
    });

    if (!verdict.ok) {
      this.discardInstaller();

      throw new UpdaterError(
        verdict.code || CODES.SIGNATURE_INVALID,
        (verdict.problems || []).join("; ") || "установщик не прошёл проверку"
      );
    }

    this.logger.info("[updater] установщик подтверждён подписанными метаданными");

    return this.installFrom(this.installerPath, { installDir: plan.installDir });
  }

  // Новая версия из чанков в workDir/tree: совпавшее берётся из установленной
  // версии, недостающие куски скачиваются. null — чанки не сработали, нужен
  // полный установщик.
  async prepareTree(delta) {
    const plan = this.plan;
    const tree = path.join(this.workDir, "tree");
    const fs = require("fs");
    this.abort = new AbortController();
    try {
      this.setStage(STAGES.DOWNLOAD, { done: 0, total: 0, bytesPerSecond: 0 });
      fs.rmSync(tree, { recursive: true, force: true });
      const manifest = await this.fetchManifest(delta, {
        isAllowedUrl: channel.isAllowedUrl, version: plan.targetVersion, platform: "win32",
        signal: this.abort.signal, fetch: this.fetchChunk,
      });
      fs.mkdirSync(tree, { recursive: true });
      const result = await this.assemble({
        manifest, sourceRoot: plan.installDir, targetDir: tree, cacheDir: path.join(this.workDir, "chunks"),
        chunkBaseUrls: delta.chunkBaseUrls, isAllowedUrl: channel.isAllowedUrl, fetch: this.fetchChunk,
        signal: this.abort.signal, log: this.logger,
        onProgress: (progress) => this.emit("progress", progress),
      });
      if (this.cancelRequested) throw new UpdaterError(CODES.CANCELLED, "отменено");
      this.logger.info(`[updater] ${plan.targetVersion} собрана из чанков: скачано ${result.downloadedBytes} байт, с диска ${result.reusedBytes}`);
      return tree;
    } catch (error) {
      if (this.cancelRequested) throw new UpdaterError(CODES.CANCELLED, "отменено");
      this.logger.warn(`[updater] чанки не сработали, качаю установщик целиком: ${error.message}`);
      fs.rmSync(tree, { recursive: true, force: true });
      return null;
    } finally {
      this.abort = null;
    }
  }

  async installFrom(installer, installOptions) {
    const plan = this.plan;

    this.setStage(STAGES.WAIT_EXIT);

    await this.waitForExit({
      imageName: "HLSR.exe",
      onWait: (info) => this.emit("waiting", info),
    });

    this.logger.info("[updater] все процессы HLSR завершены");

    this.setStage(STAGES.INSTALL);

    await this.runInstaller(installer, installOptions);

    const applied = this.verifyInstalled({
      launcherExe: plan.launcherExe,
      targetVersion: plan.targetVersion,
    });

    if (!applied.ok)
      throw new UpdaterError(
        applied.code || CODES.INSTALL_NOT_APPLIED,
        applied.detail || "установка не применилась"
      );

    this.logger.info(`[updater] установлена версия ${plan.targetVersion}`);

    const receiptWritten = this.writeReceipt(
      plan.receiptFile,
      plan.currentVersion,
      plan.targetVersion
    );

    if (!receiptWritten)
      this.logger.warn("[updater] receipt не записан: уведомление не появится");

    this.setStage(STAGES.RELAUNCH);
    this.relaunch(plan.launcherExe, {});

    this.setStage(STAGES.DONE, { version: plan.targetVersion });
    this.emit("done", { version: plan.targetVersion });

    return { ok: true, version: plan.targetVersion, receiptWritten };
  }

  discardInstaller() {
    if (!this.installerPath) return;

    try {
      require("fs").unlinkSync(this.installerPath);
    } catch (e) {
      /* файла может уже не быть */
    }
  }
}

module.exports = UpdateSession;
module.exports.STAGES = STAGES;
module.exports.CANCELLABLE_STAGES = CANCELLABLE_STAGES;
