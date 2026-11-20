"use strict";

const { EventEmitter } = require("events");

const patchChain = require("./patch-chain");
const updateTarget = require("./update-target");
const semver = require("./semver");
const ManifestService = require("./ManifestService");
const { toDeliveryError } = require("./errors");

const MIN_INTERVAL_MS = 10 * 60 * 1000;

class UpdateChecker extends EventEmitter {
  constructor(options) {
    const opts = options || {};
    super();

    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.channelService = opts.channelService || null;
    this.originUpdates = opts.originUpdates || null;
    this.installedVersionOf = opts.installedVersionOf || (() => null);
    this.localManifestOf = opts.localManifestOf || (() => null);
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.minIntervalMs = opts.minIntervalMs == null ? MIN_INTERVAL_MS : opts.minIntervalMs;
    this.now = opts.now || (() => Date.now());

    this.results = new Map();
    this.inFlight = new Map();
    this.lastCheckAt = new Map();
  }

  lastResult(gameId) {
    return this.results.get(gameId) || null;
  }

  isDue(gameId) {
    const last = this.lastCheckAt.get(gameId);
    if (!last) return true;
    return this.now() - last >= this.minIntervalMs;
  }

  isChecking(gameId) {
    return this.inFlight.has(gameId);
  }

  check(gameId, options) {
    const opts = options || {};

    const active = this.inFlight.get(gameId);
    if (active) return active;

    if (!opts.force && !this.isDue(gameId)) {
      const cached = this.lastResult(gameId);
      if (cached) return Promise.resolve(Object.assign({}, cached, { fromCache: true }));
    }

    const promise = Promise.resolve()
      .then(() => this.run(gameId, opts))
      .finally(() => this.inFlight.delete(gameId));

    this.inFlight.set(gameId, promise);
    this.emit("checking", { gameId, reason: opts.reason || null });

    return promise;
  }

  async run(gameId, opts) {
    const installedVersion = this.installedVersionOf(gameId);
    const previous = this.lastResult(gameId);

    this.lastCheckAt.set(gameId, this.now());

    if (!installedVersion) {
      return this.publish(gameId, {
        gameId,
        installed: false,
        installedVersion: null,
        availableVersion: null,
        upToDate: true,
        updateAvailable: false,
        updateRequired: false,
        kind: null,
        totalSize: 0,
        steps: [],
        source: null,
        checkedAt: new Date().toISOString(),
        error: null,
      });
    }

    let target = null;
    let error = null;
    let channelUnreachable = false;

    if (this.channelService) {
      try {
        const fetched = await this.channelService.fetch(gameId, {
          ignoreCache: !!opts.ignoreCache,
        });

        target = updateTarget.fromChannel(fetched.manifest);

        if (target) target.manifestSha256 = ManifestService.hash(fetched.manifest);
      } catch (fetchError) {
        const delivery = toDeliveryError(fetchError, "CHANNEL_UNAVAILABLE");

        if (delivery.code !== "CHANNEL_DISABLED") {
          error = delivery.toPublic();
          channelUnreachable = true;

          this.logger.warn(
            `[game-delivery] проверка обновлений ${gameId} не удалась: ` +
              `${delivery.code} ${delivery.message}`
          );
        }
      }
    }

    if (channelUnreachable)
      return this.publish(gameId, this.degrade(gameId, installedVersion, previous, error));

    if (!target && this.originUpdates && this.originUpdates.supports(gameId)) {
      try {
        target = await this.originUpdates.target(gameId, this.localManifestOf(gameId));
      } catch (originError) {
        const delivery = toDeliveryError(originError, "CHANNEL_UNAVAILABLE");
        this.logger.warn(
          `[game-delivery] проверка источника ${gameId} не удалась: ${delivery.code} ${delivery.message}`
        );
        return this.publish(gameId, this.degrade(gameId, installedVersion, previous, delivery.toPublic()));
      }
    }

    if (!target) target = updateTarget.fromLocal(this.localManifestOf(gameId));

    if (!target) return this.publish(gameId, this.degrade(gameId, installedVersion, previous, error));

    if (
      target.minimumLauncherVersion &&
      semver.lt(this.launcherVersion, target.minimumLauncherVersion)
    ) {
      return this.publish(gameId, {
        gameId,
        installed: true,
        installedVersion,
        availableVersion: target.version,
        upToDate: false,
        updateAvailable: false,
        updateRequired: false,
        launcherTooOld: true,
        kind: null,
        totalSize: 0,
        steps: [],
        source: target.source,
        checkedAt: new Date().toISOString(),
        error: { code: "LAUNCHER_TOO_OLD", message: `Нужен лаунчер ${target.minimumLauncherVersion}` },
      });
    }

    const plan = patchChain.planUpdate({
      patches: target.patches,
      fromVersion: installedVersion,
      toVersion: target.version,
      fullSize: target.fullArchive ? target.fullArchive.size : 0,
    });

    const upToDate = plan.kind === "up-to-date";

    const downgrade = plan.reason === "downgrade";
    const updateAvailable = !upToDate && !downgrade;

    return this.publish(gameId, {
      gameId,
      installed: true,
      installedVersion,
      availableVersion: target.version,
      upToDate: upToDate || downgrade,
      updateAvailable,
      downgrade,
      updateRequired: updateAvailable && !!target.updateRequired,
      kind: upToDate ? null : plan.kind,
      reason: plan.reason || null,
      totalSize: plan.totalSize || 0,
      steps: plan.steps.map((step) => ({
        fromVersion: step.fromVersion,
        toVersion: step.toVersion,
        size: step.archive ? step.archive.size : 0,
      })),
      source: target.source,
      publishedAt: target.publishedAt || null,
      checkedAt: new Date().toISOString(),
      target,
      error,
    });
  }

  degrade(gameId, installedVersion, previous, error) {
    if (previous && previous.installed)
      return Object.assign({}, previous, {
        stale: true,
        checkedAt: new Date().toISOString(),
        error,
      });

    return {
      gameId,
      installed: true,
      installedVersion,
      availableVersion: null,
      upToDate: true,
      updateAvailable: false,
      updateRequired: false,
      unknown: true,
      kind: null,
      totalSize: 0,
      steps: [],
      source: null,
      checkedAt: new Date().toISOString(),
      error,
    };
  }

  publish(gameId, result) {
    this.results.set(gameId, result);

    this.inFlight.delete(gameId);

    this.emit("result", result);
    return result;
  }

  checkIfOlderThan(gameId, maxAgeMs, reason) {
    const last = this.lastCheckAt.get(gameId);
    if (last && this.now() - last < maxAgeMs) {
      const cached = this.lastResult(gameId);
      return Promise.resolve(cached ? Object.assign({}, cached, { fromCache: true }) : null);
    }

    return this.check(gameId, { force: true, reason: reason || "focus" });
  }

  invalidate(gameId) {
    this.results.delete(gameId);
    this.lastCheckAt.delete(gameId);
  }
}

module.exports = UpdateChecker;
module.exports.MIN_INTERVAL_MS = MIN_INTERVAL_MS;
