"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { EventEmitter } = require("events");

const ManifestService = require("./ManifestService");
const GameDownloadManager = require("./GameDownloadManager");
const InstallService = require("./InstallService");
const ArchiveService = require("./ArchiveService");
const PatchService = require("./PatchService");
const IntegrityService = require("./IntegrityService");
const RepairService = require("./RepairService");
const GameProcessService = require("./GameProcessService");
const ChannelService = require("./ChannelService");
const UpdateChecker = require("./UpdateChecker");
const OriginUpdates = require("./origin-updates");
const FilesManifestStore = require("./FilesManifestStore");
const filesManifestLib = require("./files-manifest");
const patchFormat = require("./patch-format");
const { createMatcher } = require("./path-match");
const patchChain = require("./patch-chain");
const updateTarget = require("./update-target");
const mainButton = require("./main-button");
const unavailableReasons = require("./unavailable-reasons");
const buildMetadataTrust = require("./build-metadata-trust");
const providers = require("./providers");
const { MirrorRunner } = require("./mirror-runner");
const featureFlag = require("./feature-flag");
const manifestRegistry = require("./manifests");
const repairMirror = require("./repair-mirror");
const gameRemoval = require("./game-removal");
const DepotInstaller = require("./depot-install");
const gamePack = require("./game-pack");
const fileOps = require("./file-ops");
const episodes = require("./episodes");
const episodeInstall = require("./episode-install");
const semver = require("./semver");

// Манифест эпизода для размера в «Версии сборки» — не чаще раза в 10 минут; сервер не ответил —
// следующая попытка через минуту, а не на каждое открытие.
const EPISODE_MANIFEST_TTL_MS = 10 * 60 * 1000;
const EPISODE_MANIFEST_RETRY_MS = 60 * 1000;
// Отказы, которые доказывают, что скачанный архив эпизода плохой: его можно удалить. При прочих
// (игра запущена, отмена, нет места, файл занят) он остаётся, и повтор обходится без загрузки.
const EPISODE_ARCHIVE_BAD = ["CHECKSUM_MISMATCH", "ARCHIVE_UNSAFE_ENTRY", "ARCHIVE_LIMIT_EXCEEDED", "EXTRACT_FAILED", "MANIFEST_INVALID"];

function versionEventsSilenced(service, gameId) {
  return !!(service.silentVersionEvents && service.silentVersionEvents.has(String(gameId)));
}

const { resolveLibrary, ensureLayout, assertInside, removeWithinZone, gameInstallPath } = require("./paths");
const { DeliveryError, toDeliveryError } = require("./errors");

const STATES = [
  "not-installed",
  "preparing",
  "downloading",
  "paused",
  "verifying",
  "extracting",
  "finalizing",
  "applying-update",
  "checking-files",
  "files-valid",
  "files-damaged",
  "repairing",
  "repair-complete",
  "exporting",
  "importing",
  "ready",
  "cancelled",
  "error",
];


class GameDeliveryService extends EventEmitter {
  constructor(options) {
    super();
    const opts = options || {};

    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.env = opts.env || process.env;
    this.launcherVersion = opts.launcherVersion || "0.0.0";

    this.paths = resolveLibrary(opts.libraryPath);

    this.registry = opts.manifests || manifestRegistry.MANIFESTS;

    this.filesRegistry = opts.filesManifests || manifestRegistry.FILES_MANIFESTS;

    this.depotService = opts.depotService || null;

    this.depotInstaller = opts.depotService
      ? new DepotInstaller({
          paths: this.paths,
          depotService: opts.depotService,
          logger: this.logger,
          setRegistryValue: opts.setRegistryValue || null,
        })
      : null;

    this.filesStore =
      opts.filesStore ||
      new FilesManifestStore({ paths: this.paths, logger: this.logger, env: this.env });

    this.manifestService = new ManifestService({ launcherVersion: this.launcherVersion });

    this.mirrors = opts.mirrors || new MirrorRunner({ logger: this.logger });
    this.lastMirrorAttempts = [];
    this.archiveService = opts.archiveService || new ArchiveService({ logger: this.logger });

    this.downloadManager =
      opts.downloadManager ||
      new GameDownloadManager({
        paths: this.paths,
        logger: this.logger,
        timeout: opts.timeout,
        maxAttempts: opts.maxAttempts,
        backoffBaseMs: opts.backoffBaseMs,
        freeSpaceProvider: opts.freeSpaceProvider,
        rateLimiter: opts.rateLimiter,
      });

    this.installService =
      opts.installService ||
      new InstallService({
        paths: this.paths,
        archiveService: this.archiveService,
        logger: this.logger,
        launcherVersion: this.launcherVersion,
      });

    this.patchService =
      opts.patchService ||
      new PatchService({
        paths: this.paths,
        archiveService: this.archiveService,
        logger: this.logger,
        launcherVersion: this.launcherVersion,
      });

    this.integrityService = opts.integrityService || new IntegrityService({ logger: this.logger });

    this.repairService =
      opts.repairService ||
      new RepairService({
        paths: this.paths,
        logger: this.logger,
      });

    this.repairMirror =
      opts.repairMirror || new repairMirror.RepairMirrorClient({
        logger: this.logger,
        rateLimiter: opts.rateLimiter,
      });

    this.processService =
      opts.processService ||
      new GameProcessService({
        paths: this.paths,
        logger: this.logger,
        stopTimeoutMs: opts.stopTimeoutMs,
      });

    this.channelService =
      opts.channelService === null
        ? null
        : opts.channelService ||
          new ChannelService({ logger: this.logger, env: this.env, freshnessFile: opts.channelFreshnessFile });

    this.originUpdates =
      opts.originUpdates === null ? null : opts.originUpdates || new OriginUpdates({ logger: this.logger });

    this.updateChecker =
      opts.updateChecker ||
      new UpdateChecker({
        logger: this.logger,
        channelService: this.channelService,
        originUpdates: this.originUpdates,
        launcherVersion: this.launcherVersion,
        minIntervalMs: opts.updateCheckIntervalMs,
        installedVersionOf: (gameId) => this.installedVersionOf(gameId),
        localManifestOf: (gameId) => {
          try {
            return this.manifestFor(gameId).manifest;
          } catch (error) {
            return null;
          }
        },
      });

    this.versionEvents = opts.versionEvents || null;
    this.releaseNotes = opts.releaseNotes || null;

    this.statuses = new Map();
    this.activeGameId = null;
    this.slot = null;
    this.lastIntegrity = new Map();
    // Отмена идущего экспорта или импорта сборки (exportPack, importPack).
    this.packCancel = null;
    // Отмена загрузки файлов с зеркала ремонта (fetchFromMirror).
    this.repairAbort = null;

    this.silentVersionEvents = new Set();
    this.operations = new Map();
    this.publishedAtBackfilled = new Set();
    // Последние принятые манифесты эпизодов: id → { manifest, url, fetchedAt }; отказы: id → { error, at }.
    this.episodeManifests = new Map();
    this.episodeManifestFailures = new Map();

    this.processService.on("state", (event) => this.onProcessState(event));
    // Вышел эпизод (и подхваченный после перезапуска лаунчера): его config.cfg — в Ghosting.
    this.processService.on("finished", (event) => {
      if (event.episode) this.returnEpisodeConfigs(event.gameId, [event.episode]);
    });
    this.updateChecker.on("checking", (event) => this.refresh(event.gameId));
    this.updateChecker.on("result", (event) => {
      this.refresh(event.gameId);
      this.announceUpdateResult(event);
    });

    this.downloadManager.on("progress", (progress) => this.onDownloadProgress(progress));
    this.downloadManager.on("state", (event) => this.onDownloadState(event));
    this.installService.on("state", (event) => this.onInstallState(event));
    this.patchService.on("state", (event) => this.onPatchState(event));
    this.integrityService.on("progress", (event) => this.onIntegrityProgress(event));
    this.repairService.on("state", (event) => this.onRepairState(event));
    this.repairService.on("progress", (event) => this.onIntegrityProgress(event));
  }

  filesManifestFor(gameId, version) {
    if (!version)
      throw new DeliveryError(
        "FILES_MANIFEST_MISSING",
        `Не задана версия сборки ${gameId} — неизвестно, каким манифестом проверять`,
        { gameId }
      );

    const bundled = this.bundledFilesManifest(gameId, version);
    if (bundled) return bundled;

    const cached = this.filesStore.load(gameId, String(version));
    if (cached) return cached;

    throw new DeliveryError(
      "FILES_MANIFEST_MISSING",
      `Нет файлового манифеста версии ${version} для ${gameId}`,
      {
        gameId,
        version,
        bundled: Object.keys(this.filesRegistry[gameId] || {}),
        cached: this.filesStore.list(gameId),
      }
    );
  }

  bundledFilesManifest(gameId, version) {
    const byVersion = this.filesRegistry[gameId] || null;
    const manifest = byVersion ? byVersion[String(version)] || null : null;

    if (!manifest) return null;

    return filesManifestLib.validate(manifest, `bundled:${gameId}-${version}.files.json`, {
      gameId,
      version: String(version),
    });
  }

  hasFilesManifest(gameId, version) {
    try {
      this.filesManifestFor(gameId, version);
      return true;
    } catch (error) {
      return false;
    }
  }

  async ensureFilesManifest(gameId, version, options) {
    const opts = options || {};

    if (!version)
      throw new DeliveryError("FILES_MANIFEST_MISSING", "Не задана версия сборки", { gameId });

    if (!opts.force && this.hasFilesManifest(gameId, version))
      return { status: "already-available", version };

    if (!this.channelService)
      throw new DeliveryError("CHANNEL_DISABLED", "Канал обновлений выключен", { gameId });

    const fetched = await this.channelService.fetch(gameId, { ignoreCache: !!opts.ignoreCache });
    const descriptor = fetched.manifest.filesManifest;

    if (!descriptor || descriptor.version !== String(version))
      throw new DeliveryError(
        "FILES_MANIFEST_MISSING",
        `Канал объявляет файловый манифест версии ${descriptor && descriptor.version}, ` +
          `а нужен ${version}`,
        { gameId, version }
      );

    const manifest = await this.filesStore.fetch(gameId, descriptor, {
      channelSha256: ManifestService.hash(fetched.manifest),
    });

    return { status: "stored", version, totalFiles: manifest.totalFiles };
  }

  async tryEnsureFilesManifest(gameId, version) {
    try {
      return await this.ensureFilesManifest(gameId, version);
    } catch (error) {
      const delivery = toDeliveryError(error, "FILES_MANIFEST_MISSING");

      this.logger.warn(
        `[game-delivery] файловый манифест ${gameId} ${version} не получен: ` +
          `${delivery.code} ${delivery.message}`
      );

      return { status: "failed", version, error: delivery.toPublic() };
    }
  }

  async resolveFilesManifest(gameId, version) {
    if (this.hasFilesManifest(gameId, version)) return this.filesManifestFor(gameId, version);

    await this.tryEnsureFilesManifest(gameId, version);

    return this.filesManifestFor(gameId, version);
  }

  installedVersionOf(gameId) {
    const installed = this.installService.readInstalled(gameId);
    return installed ? installed.version : null;
  }

  buildDescriptorFor(gameId, version) {
    let bundled = null;
    try {
      bundled = this.manifestFor(gameId).manifest;
    } catch (error) {
      return null;
    }

    if (!version) return null;

    const target = String(version);

    const cached = this.channelService ? this.channelService.cachedFor(gameId) : null;
    const channel = cached && cached.manifest ? cached.manifest : null;

    if (
      channel &&
      channel.buildMetadata &&
      channel.buildMetadata.buildId != null &&
      channel.gameId === bundled.gameId &&
      String(channel.version) === target
    )
      return {
        descriptor: channel.buildMetadata,
        manifest: Object.assign({}, bundled, {
          version: channel.version,
          publishedAt: channel.publishedAt || bundled.publishedAt || null,
          buildMetadata: channel.buildMetadata,
        }),
        source: "channel",
      };

    if (bundled.buildMetadata && bundled.buildMetadata.buildId != null && bundled.version === target)
      return { descriptor: bundled.buildMetadata, manifest: bundled, source: "bundled" };

    return null;
  }

  refreshBuildMetadataSnapshot(installed) {
    const gameId = installed.gameId;
    const resolved = this.buildDescriptorFor(gameId, installed.version);

    if (!resolved) return null;

    let verified;
    try {
      verified = buildMetadataTrust.verifyForInstall({
        installRoot: installed.installPath,
        descriptor: resolved.descriptor,
        manifest: resolved.manifest,
        launcherVersion: this.launcherVersion,
      });
    } catch (error) {
      this.logger.info(
        `[game-delivery] снимок сведений о сборке ${gameId} освежить не вышло (${error.code || "?"}): ${error.message}`
      );
      return null;
    }

    const snapshot = buildMetadataTrust.snapshotFromVerified(verified);

    try {
      this.installService.replaceBuildMetadataSnapshot(gameId, snapshot);
    } catch (error) {
      this.logger.warn(`[game-delivery] снимок сведений о сборке не записан: ${error.message}`);
      return null;
    }

    this.logger.info(
      `[game-delivery] снимок сведений о сборке ${gameId} обновлён до ${snapshot.buildMetadata.version} ` +
        `(${snapshot.buildMetadata.buildId}) — запись отставала от установки`
    );

    installed.buildMetadata = snapshot.buildMetadata;
    installed.buildMetadataSha256 = snapshot.buildMetadataSha256;
    installed.buildMetadataVerifiedAt = snapshot.buildMetadataVerifiedAt;

    return { state: "trusted", reason: null, metadata: verified.metadata };
  }

  computeBuildMetadataView(installed) {
    if (!installed)
      return {
        buildMetadata: null,
        buildDate: null,
        buildDateSource: null,
        metadataState: "not-installed",
        metadataIssue: null,
      };

    let snapshotMeta = installed.buildMetadata || null;

    let buildDate = null;
    let buildDateSource = null;
    if (snapshotMeta && snapshotMeta.builtAt) {
      buildDate = snapshotMeta.builtAt;
      buildDateSource = "builtAt";
    } else if (installed.publishedAt) {
      buildDate = installed.publishedAt;
      buildDateSource = "publishedAt";
    }

    let metadataState = snapshotMeta ? "trusted" : "legacy";
    let metadataIssue = null;

    if (snapshotMeta) {
      let runtime;
      try {
        runtime = buildMetadataTrust.verifyAgainstSnapshot({
          installRoot: installed.installPath,
          snapshot: installed,
          launcherVersion: this.launcherVersion,
        });
      } catch (error) {
        this.logger.warn(`[game-delivery] проверка метаданных сборки не удалась: ${error.message}`);
        runtime = { state: "repair-required", reason: buildMetadataTrust.REASONS.INVALID };
      }

      if (
        runtime.state === "repair-required" &&
        snapshotMeta.version &&
        installed.version &&
        snapshotMeta.version !== installed.version
      ) {
        const refreshed = this.refreshBuildMetadataSnapshot(installed);

        if (refreshed) {
          runtime = refreshed;

          snapshotMeta = installed.buildMetadata || snapshotMeta;

          if (snapshotMeta && snapshotMeta.builtAt) {
            buildDate = snapshotMeta.builtAt;
            buildDateSource = "builtAt";
          }
        }
      }

      metadataState = runtime.state;
      if (runtime.reason) metadataIssue = { code: runtime.reason, detail: runtime.detail || null };
    }

    return {
      buildMetadata: snapshotMeta
        ? {
            version: snapshotMeta.version,
            buildId: snapshotMeta.buildId,
            buildName: snapshotMeta.buildName,
            gitCommit: snapshotMeta.gitCommit,
            builtAt: snapshotMeta.builtAt,
            architecture: snapshotMeta.architecture,
            minimumLauncherVersion: snapshotMeta.minimumLauncherVersion,
          }
        : null,
      buildDate,
      buildDateSource,
      metadataState,
      metadataIssue,
    };
  }

  manifestFor(gameId) {
    const manifest = this.registry[gameId];
    if (!manifest) throw new DeliveryError("MANIFEST_NOT_FOUND", `Нет манифеста для ${gameId}`);

    return this.manifestService.validate(manifest, `bundled:${gameId}`);
  }

  defaultStatus(gameId) {
    return {
      gameId,
      launcherId: manifestRegistry.launcherIdOf(gameId),
      // Окно «Почему недоступно» предлагает ремонт только таким играм.
      fileRepair: manifestRegistry.isFileRepairGame(gameId),
      state: "not-installed",
      percent: 0,
      bytesDownloaded: 0,
      totalBytes: 0,
      speedBps: 0,
      etaSeconds: null,
      version: null,
      installedVersion: null,
      installedAt: null,
      buildMetadata: null,
      buildDate: null,
      buildDateSource: null,
      metadataState: null,
      metadataIssue: null,
      installable: false,
      reasons: [],
      unavailableReason: null,
      error: null,
      pending: null,
      phase: null,
      checked: 0,
      totalChecked: 0,
      integrity: null,
      updatePlan: null,
      source: null,
      operation: null,
      chain: null,
      process: this.processService ? this.processService.getProcessState(gameId) : null,
      update: null,
      mainButton: null,
      // Эпизод, который ставит операция episode-install (ghosting-renewed).
      episode: null,
    };
  }

  statusOf(gameId) {
    if (!this.statuses.has(gameId)) this.statuses.set(gameId, this.defaultStatus(gameId));
    return this.statuses.get(gameId);
  }

  setStatus(gameId, patch) {
    const status = Object.assign(this.statusOf(gameId), patch);

    status.operation = this.operations.get(gameId) || null;
    status.process = this.processService.getProcessState(gameId);
    status.update = this.publicUpdate(gameId);
    status.unavailableReason = unavailableReasons.fromDeliveryStatus(status);
    status.mainButton = mainButton.resolve({
      delivery: status,
      process: status.process,
      update: status.update,
      checkingUpdates: this.updateChecker.isChecking(gameId),
    });

    this.statuses.set(gameId, status);
    this.emit("status", status);
    return status;
  }

  publicUpdate(gameId) {
    const result = this.updateChecker.lastResult(gameId);
    if (!result) return null;

    return {
      installedVersion: result.installedVersion,
      availableVersion: result.availableVersion,
      upToDate: !!result.upToDate,
      updateAvailable: !!result.updateAvailable,
      updateRequired: !!result.updateRequired,
      kind: result.kind || null,
      totalSize: result.totalSize || 0,
      steps: (result.steps || []).map((step) => `${step.fromVersion} -> ${step.toVersion}`),
      source: result.source || null,
      checkedAt: result.checkedAt || null,
      stale: !!result.stale,
      unknown: !!result.unknown,
      error: result.error || null,
    };
  }

  async downloadWithFallback(request) {
    const { gameId, descriptors, filename, expectedSize, expectedSha256 } = request;
    const allowedProviderTypes = featureFlag.allowedProviderTypes(this.env);

    const usable = (descriptors || []).filter(
      (descriptor) => allowedProviderTypes.indexOf(descriptor.type) !== -1
    );

    if (!usable.length)
      throw new DeliveryError(
        "REMOTE_DISABLED",
        request.disabledMessage || "Источник загрузки выключен feature flag'ом"
      );

    const outcome = await this.mirrors.run({
      artifact: gameId + ":" + filename,
      descriptors: ManifestService.orderProviders(usable),
      isTerminalResult: (value) => !!value && value.status !== "completed",
      onDiscardPartial: () => this.downloadManager.discard(gameId, filename),
      operation: async ({ descriptor }) => {
        const provider = providers.create(descriptor, {
          env: this.env,
          allowedProviderTypes,
        });

        return this.downloadManager.start({
          gameId,
          provider,
          filename,
          expectedSize,
          expectedSha256,
        });
      },
    });

    this.lastMirrorAttempts = outcome.attempts;
    if (outcome.usedFallback)
      this.logger.warn(
        "[game-delivery] " + filename + ": использован резервный источник " + outcome.providerId
      );

    return outcome.value;
  }

  mirrorDiagnostics() {
    return {
      attempts: this.lastMirrorAttempts.slice(),
      cooldown: this.mirrors.cooldown.snapshot(),
    };
  }

  setOperation(gameId, operation) {
    if (operation) this.operations.set(gameId, operation);
    else this.operations.delete(gameId);
  }

  onProcessState(event) {
    if (!event || !event.gameId) return;
    this.refresh(event.gameId);
  }

  resolveUpdateOffers(gameId, installedVersion) {
    try {
      const events = this.versionEvents;
      if (events && typeof events.resolveUpdateOffers === "function")
        events.resolveUpdateOffers("game", installedVersion, gameId);
    } catch (error) {
      this.logger.warn(`[game-delivery] приглашения не пересмотрены: ${error.message}`);
    }
  }

  announceUpdateResult(result) {
    if (!this.versionEvents || !result || !result.gameId) return;
    if (versionEventsSilenced(this, result.gameId)) return;
    if (result.stale || result.unknown) return;

    this.resolveUpdateOffers(result.gameId, result.installedVersion);

    if (!result.updateAvailable) return;
    if (!result.availableVersion || !result.installed) return;

    let hasNotes = false;
    try {
      if (this.releaseNotes) {
        if (result.target && result.target.notes)
          this.releaseNotes.capture(result.gameId, result.availableVersion, result.target.notes);
        hasNotes = this.releaseNotes.has(result.gameId, result.availableVersion);
      }
    } catch (error) {
      this.logger.warn(`[game-delivery] патчноут не сохранён: ${error.message}`);
    }

    try {
      this.versionEvents.gameUpdateAvailable(
        result.availableVersion,
        result.installedVersion,
        result.gameId,
        { hasReleaseNotes: hasNotes, launcherId: manifestRegistry.launcherIdOf(result.gameId) }
      );
    } catch (error) {
      this.logger.warn(`[game-delivery] событие версии не создано: ${error.message}`);
    }
  }

  announceInstalled(gameId, installedVersion, manifestNotes) {
    if (!this.versionEvents || !installedVersion) return;
    if (versionEventsSilenced(this, gameId)) return;

    if (this.installedVersionOf(gameId) !== installedVersion) return;

    let hasNotes = false;
    try {
      if (this.releaseNotes) {
        if (manifestNotes) this.releaseNotes.capture(gameId, installedVersion, manifestNotes);
        hasNotes = this.releaseNotes.has(gameId, installedVersion);
      }
    } catch (error) {
      this.logger.warn(`[game-delivery] патчноут не сохранён: ${error.message}`);
    }

    this.resolveUpdateOffers(gameId, installedVersion);

    try {
      this.versionEvents.gameInstalled(installedVersion, gameId, {
        hasReleaseNotes: hasNotes,
        launcherId: manifestRegistry.launcherIdOf(gameId),
      });
    } catch (error) {
      this.logger.warn(`[game-delivery] событие версии не создано: ${error.message}`);
    }
  }

  announceUpdated(gameId, fromVersion, toVersion) {
    if (!this.versionEvents || !toVersion) return;
    if (versionEventsSilenced(this, gameId)) return;
    if (this.installedVersionOf(gameId) !== toVersion) return;

    let hasNotes = false;
    try {
      hasNotes = this.releaseNotes ? this.releaseNotes.has(gameId, toVersion) : false;
    } catch (error) {
      hasNotes = false;
    }

    this.resolveUpdateOffers(gameId, toVersion);

    try {
      this.versionEvents.gameUpdated(fromVersion, toVersion, gameId, {
        hasReleaseNotes: hasNotes,
        launcherId: manifestRegistry.launcherIdOf(gameId),
      });
    } catch (error) {
      this.logger.warn(`[game-delivery] событие версии не создано: ${error.message}`);
    }
  }

  ensureNotesForInstalled(gameId) {
    if (!this.releaseNotes) return;

    try {
      const installed = this.installService.readInstalled(gameId);
      if (!installed || installed.state !== "ready" || !installed.version) return;

      const declared = this.manifestFor(gameId).manifest;
      if (declared.version !== installed.version || !declared.notes) return;

      this.releaseNotes.capture(gameId, installed.version, declared.notes);
    } catch (error) {
      this.logger.warn(`[release-notes] заметка не обновлена: ${error.message}`);
    }
  }

  refresh(gameId) {
    if (manifestRegistry.isDepotBuild(gameId)) return this.refreshDepot(gameId);

    this.ensureNotesForInstalled(gameId);

    let loaded;
    try {
      loaded = this.manifestFor(gameId);
    } catch (error) {
      const delivery = toDeliveryError(error, "MANIFEST_INVALID");
      return this.setStatus(gameId, { state: "error", error: delivery.toPublic() });
    }

    const manifest = loaded.manifest;
    const check = this.manifestService.checkInstallable(manifest, {
      launcherVersion: this.launcherVersion,
      allowedProviderTypes: featureFlag.allowedProviderTypes(this.env),
    });

    const installed = this.backfillPublishedAt(gameId, this.installService.readInstalled(gameId));
    const resumable = this.downloadManager.findResumable(gameId, manifest.baseArchive.filename);
    const pendingOperation = this.installService
      .listPendingOperations()
      .find((item) => item.gameId === gameId);

    const metaView = this.computeBuildMetadataView(installed);

    const patch = {
      version: manifest.version,
      installedVersion: installed ? installed.version : null,
      installedAt: installed ? installed.installedAt || null : null,
      buildMetadata: metaView.buildMetadata,
      buildDate: metaView.buildDate,
      buildDateSource: metaView.buildDateSource,
      metadataState: metaView.metadataState,
      metadataIssue: metaView.metadataIssue,
      installable: check.ok,
      requiresLicense: this.requiresValveLicense(gameId),
      source: "http",
      reasons: check.reasons.map((reason) => reason.code),
      totalBytes: manifest.baseArchive.size,
      pending: pendingOperation || null,
    };

    if (gameId === episodeInstall.GAME_ID) {
      patch.episodes = this.episodesView(installed);
      // Идёт загрузка эпизода: её объём не подменять объёмом архива Ghosting.
      if (this.operations.get(gameId) === "episode-install") delete patch.totalBytes;
      if (this.activeGameId !== gameId) patch.episode = null;
    }

    if (this.activeGameId === gameId) return this.setStatus(gameId, patch);

    if (installed) {
      if (metaView.metadataState === "repair-required")
        return this.setStatus(gameId, Object.assign(patch, { state: "files-damaged", percent: 100 }));
      return this.setStatus(gameId, Object.assign(patch, { state: "ready", percent: 100 }));
    }

    if (resumable && resumable.bytesDownloaded > 0)
      return this.setStatus(
        gameId,
        Object.assign(patch, {
          state: "paused",
          bytesDownloaded: resumable.bytesDownloaded,
          percent: manifest.baseArchive.size
            ? (resumable.bytesDownloaded / manifest.baseArchive.size) * 100
            : 0,
        })
      );

    return this.setStatus(gameId, Object.assign(patch, { state: "not-installed", percent: 0 }));
  }

  refreshDepot(gameId) {
    const installed = this.installService.readInstalled(gameId);

    const patch = {
      version: installed ? installed.version : null,
      installedVersion: installed ? installed.version : null,
      installedAt: installed ? installed.installedAt || null : null,
      buildMetadata: null,
      buildDate: null,
      buildDateSource: null,
      metadataState: "legacy",
      metadataIssue: null,
      installable: true,
      requiresLicense: this.requiresValveLicense(gameId),
      reasons: [],
      totalBytes: 0,
      pending: null,
      source: "steam-depot",
    };

    if (this.activeGameId === gameId) return this.setStatus(gameId, patch);

    return this.setStatus(
      gameId,
      Object.assign(patch, installed ? { state: "ready", percent: 100 } : { state: "not-installed", percent: 0 })
    );
  }

  refreshAll() {
    return Object.keys(this.registry)
      .concat(Array.from(manifestRegistry.DEPOT_BUILDS))
      .map((gameId) => this.refresh(gameId));
  }

  onDownloadProgress(progress) {
    this.setStatus(progress.gameId, {
      state: "downloading",
      percent: progress.percent,
      bytesDownloaded: progress.bytesDownloaded,
      totalBytes: progress.totalBytes,
      speedBps: progress.speedBps,
      etaSeconds: progress.etaSeconds,
    });
  }

  onDownloadState(event) {
    const map = {
      preparing: "preparing",
      resuming: "downloading",
      downloading: "downloading",
      retrying: "downloading",
      paused: "paused",
      cancelled: "cancelled",
      downloaded: "verifying",
      error: "error",
    };

    const state = map[event.state];
    if (!state) return;

    this.setStatus(event.gameId, {
      state,
      percent: event.percent,
      bytesDownloaded: event.bytesDownloaded,
      totalBytes: event.totalBytes,
      speedBps: event.speedBps,
      etaSeconds: event.etaSeconds,
      error: event.error || null,
    });
  }

  onInstallState(event) {
    const patch = { state: event.state, error: event.error || null };
    if (event.state === "extracting" && typeof event.percent === "number")
      patch.percent = event.percent;
    if (event.state === "ready") patch.percent = 100;

    this.setStatus(event.gameId, patch);
  }

  onPatchState(event) {
    const gameId = event.gameId || this.activeGameId;
    if (!gameId) return;

    const patch = { state: event.state, error: event.error || null };
    if (typeof event.percent === "number") patch.percent = event.percent;
    if (event.phase) patch.phase = event.phase;

    this.setStatus(gameId, patch);
  }

  onIntegrityProgress(event) {
    const gameId = event.gameId || this.activeGameId;
    if (!gameId) return;

    this.setStatus(gameId, {
      checked: event.checked,
      totalChecked: event.total,
      percent: typeof event.percent === "number" ? event.percent : 0,
    });
  }

  onRepairState(event) {
    const gameId = event.gameId || this.activeGameId;
    if (!gameId) return;

    this.setStatus(gameId, {
      state: event.state,
      phase: event.phase || null,
      error: event.error || null,
    });
  }

  isBusy() {
    return (
      this.slot !== null ||
      this.downloadManager.isBusy() ||
      this.installService.isBusy() ||
      this.patchService.busy ||
      this.integrityService.busy ||
      this.repairService.busy
    );
  }

  /**
   * Занять слот операции. СИНХРОННО, до первого await.
   *
   * Бросает ALREADY_RUNNING, если занято. Возвращает жетон: освобождать
   * слот можно только им, и только если его никто не перехватил — иначе
   * поздний finally чужой операции снял бы чужую же пометку.
   */
  reserveSlot(gameId, operation) {
    this.assertNotRendering(gameId);
    if (this.isBusy())
      throw new DeliveryError("ALREADY_RUNNING", "Уже идёт другая операция", {
        gameId: (this.slot && this.slot.gameId) || this.activeGameId || null,
        operation: (this.slot && this.slot.operation) || null,
      });

    this.slot = { gameId, operation };

    return this.slot;
  }

  releaseSlot(token) {
    if (!token || this.slot !== token) return;
    this.slot = null;
  }

  // Инструмент «Демо» рендерит игру в фоне (renderActive ставит GameDelivery):
  // её файлы заняты, а реестр и папка мода вернутся только в конце рендера.
  assertNotRendering(gameId) {
    if (typeof this.renderActive === "function" && this.renderActive(gameId))
      throw new DeliveryError("RENDER_ACTIVE", "Идёт рендер демо этой игры", { gameId });
  }

  assertGameNotRunning(gameId) {
    this.assertNotRendering(gameId);
    if (!this.processService.isRunning(gameId)) return;

    throw new DeliveryError(
      "GAME_RUNNING",
      "Игра запущена. Закройте её и повторите операцию.",
      { gameId }
    );
  }

  publishedAtFor(gameId, version) {
    const last = this.updateChecker.lastResult(gameId);
    const target = last && last.target ? last.target : null;

    if (target && target.version === version && target.publishedAt) return target.publishedAt;

    const manifest = this.manifestFor(gameId).manifest;
    return manifest.version === version ? manifest.publishedAt || null : null;
  }

  backfillPublishedAt(gameId, installed) {
    if (!installed || installed.publishedAt) return installed;
    if (this.publishedAtBackfilled.has(gameId)) return installed;

    const publishedAt = this.publishedAtFor(gameId, installed.version);
    if (!publishedAt) return installed;

    this.publishedAtBackfilled.add(gameId);

    try {
      const updated = this.installService.fillInstalled(gameId, { publishedAt });
      if (updated) this.logger.info(`[game-delivery] дата выпуска ${gameId} дозаписана: ${publishedAt}`);
      return updated || installed;
    } catch (error) {
      this.logger.warn(`[game-delivery] дата выпуска не дозаписана: ${error.message}`);
      return installed;
    }
  }

  baseVersionOf(manifest) {
    const archive = manifest.baseArchive || manifest.fullArchive || null;
    return (archive && archive.version) || manifest.version;
  }

  baseInstallManifest(gameId, manifest) {
    const baseVersion = this.baseVersionOf(manifest);
    if (baseVersion === manifest.version) return manifest;

    const files = this.filesManifestFor(gameId, baseVersion);

    if (!files)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Манифест объявляет базовым архивом версию ${baseVersion}, но её файлового манифеста нет`,
        { gameId, version: baseVersion }
      );

    return Object.assign({}, manifest, {
      version: baseVersion,
      publishedAt: this.publishedAtFor(gameId, baseVersion) || null,
      expectedInstalledSize: files.totalSize,
      buildMetadata: null,
    });
  }

  async install(gameId) {
    const slot = this.reserveSlot(gameId, "install");

    try {
      return await this.runInstall(gameId);
    } finally {
      this.releaseSlot(slot);
    }
  }

  async runInstall(gameId) {

    this.assertGameNotRunning(gameId);

    await this.assertSteamRequirements(gameId);

    if (manifestRegistry.isDepotBuild(gameId)) return this.installFromDepot(gameId);

    const loaded = this.manifestFor(gameId);
    const declared = loaded.manifest;
    const manifest = this.baseInstallManifest(gameId, declared);

    ensureLayout(this.paths);

    const check = this.manifestService.assertInstallable(manifest, {
      launcherVersion: this.launcherVersion,
      allowedProviderTypes: featureFlag.allowedProviderTypes(this.env),
    });

    const archiveProviders = check.providers;

    this.activeGameId = gameId;
    this.setOperation(gameId, "install");
    this.setStatus(gameId, { state: "preparing", error: null });

    try {
      const download = await this.downloadWithFallback({
        gameId,
        descriptors: archiveProviders,
        filename: manifest.baseArchive.filename,
        expectedSize: manifest.baseArchive.size,
        expectedSha256: manifest.baseArchive.sha256,
        disabledMessage: "Источник полного архива выключен feature flag'ом",
      });

      if (download.status !== "completed") {
        this.refresh(gameId);
        return download;
      }

      const installed = await this.installService.install({
        manifest,
        manifestSha256: loaded.sha256,
        archivePath: download.filePath,
        publishedAt: this.publishedAtFor(gameId, manifest.version),
      });

      this.downloadManager.discard(gameId, manifest.baseArchive.filename);
      this.removeArchive(gameId, manifest.baseArchive.filename);

      this.activeGameId = null;
      this.setOperation(gameId, null);
      this.refresh(gameId);

      this.announceInstalled(
        gameId,
        installed.version,
        installed.version === declared.version ? declared.notes : null
      );

      return { status: "installed", installed, baseOnly: installed.version !== declared.version };
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");
      this.setStatus(gameId, {
        state: delivery.code === "CANCELLED" ? "cancelled" : "error",
        error: delivery.toPublic(),
      });
      throw delivery;
    } finally {
      this.activeGameId = null;
      this.setOperation(gameId, null);
    }
  }

  requiresValveLicense(gameId) {
    const config = manifestRegistry.launchConfigOf(gameId);
    return !!(config && (config.requiresOwnedAppIds || []).length);
  }

  async assertSteamRequirements(gameId) {
    const config = manifestRegistry.launchConfigOf(gameId);
    const appIds = (config && config.requiresOwnedAppIds) || [];

    if (!appIds.length) return null;

    if (!this.depotService)
      throw new DeliveryError(
        "LICENSE_CHECK_UNAVAILABLE",
        "Проверка лицензии Steam недоступна в этой сборке лаунчера",
        { gameId }
      );

    await this.depotService.assertOwns(appIds);
    return { owned: true };
  }

  async installFromDepot(gameId) {
    if (!this.depotInstaller)
      throw new DeliveryError(
        "PROVIDER_UNSUPPORTED",
        "Установка через депоты Steam в этой сборке лаунчера недоступна",
        { gameId }
      );

    ensureLayout(this.paths);

    this.activeGameId = gameId;
    this.setOperation(gameId, "install");
    this.setStatus(gameId, {
      state: "preparing",
      percent: 0,
      error: null,
      source: "steam-depot",
      filesDone: 0,
      filesTotal: 0,
    });

    try {
      const installed = await this.depotInstaller.install(gameId, {
        onProgress: (progress) => {
          const total = progress.expectedFiles || 0;
          const percent = total ? Math.min(99, Math.round((progress.files / total) * 100)) : 0;

          this.setStatus(gameId, {
            state: "downloading",
            percent,
            source: "steam-depot",
            filesDone: progress.files,
            filesTotal: progress.expectedFiles || 0,
            depotStep: progress.index + 1,
            depotSteps: progress.total,
          });
        },
      });

      this.activeGameId = null;
      this.setOperation(gameId, null);
      this.setStatus(gameId, { state: "ready", percent: 100, chain: null, error: null });
      this.refresh(gameId);
      this.announceInstalled(gameId, installed.version, null);

      return { status: "installed", installed };
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");

      this.setStatus(gameId, {
        state: delivery.code === "CANCELLED" ? "cancelled" : "error",
        error: delivery.toPublic(),
        chain: null,
      });

      throw delivery;
    } finally {
      this.activeGameId = null;
      this.setOperation(gameId, null);
    }
  }

  async installToLatest(gameId) {
    this.silentVersionEvents.add(String(gameId));

    try {
      return await this.installToLatestSilently(gameId);
    } finally {
      this.silentVersionEvents.delete(String(gameId));
      this.announceInstalledFinal(gameId);
    }
  }

  announceInstalledFinal(gameId) {
    const installed = this.installService.readInstalled(gameId);
    if (!installed || installed.state !== "ready" || !installed.version) return;

    let notes = null;
    try {
      const declared = this.manifestFor(gameId).manifest;
      if (declared.version === installed.version) notes = declared.notes || null;
    } catch (error) {
      notes = null;
    }

    this.announceInstalled(gameId, installed.version, notes);
  }

  async installToLatestSilently(gameId) {
    const result = await this.install(gameId);

    if (!result || result.status !== "installed" || !result.baseOnly) return result;

    const declared = this.manifestFor(gameId).manifest.version;

    this.updateChecker.invalidate(gameId);

    try {
      await this.checkForUpdates(gameId, { force: true });
    } catch (error) {
      this.logger.warn(
        `[game-delivery] база ${gameId} установлена, но проверка обновлений не удалась: ${error.message}`
      );
      return Object.assign({}, result, { pendingVersion: declared });
    }

    const plan = this.planUpdate(gameId);

    if (plan.kind === "up-to-date") return result;

    if (plan.kind !== "patches") {
      this.logger.warn(
        `[game-delivery] от базы ${gameId} нет патча до ${declared} (${plan.reason || plan.kind}) — ` +
          `остаёмся на базе, полный архив второй раз не качаем`
      );
      return Object.assign({}, result, { pendingVersion: declared });
    }

    const updated = await this.applyUpdate(gameId);

    return {
      status: "installed",
      installed: this.installService.readInstalled(gameId),
      base: result.installed,
      updated,
    };
  }

  removeArchive(gameId, filename) {
    const target = path.join(this.paths.downloads, gameId, path.basename(filename));

    try {
      if (fs.existsSync(target)) removeWithinZone(this.paths.downloads, target);
    } catch (e) {
      this.logger.warn(`[game-delivery] не удалось удалить архив: ${e.message}`);
    }
  }

  installedOf(gameId) {
    const installed = this.installService.readInstalled(gameId);

    if (!installed)
      throw new DeliveryError("NOT_INSTALLED", "Игра не установлена", { gameId });

    return installed;
  }

  assertNotDepotBuild(gameId, action) {
    if (!manifestRegistry.isDepotBuild(gameId)) return;

    throw new DeliveryError(
      "STEAM_MANAGED_BUILD",
      `Сборка ${gameId} доставляется Steam: ${action} для неё не выполняется`,
      { gameId }
    );
  }

  // Проверка и ремонт по файлам включены по списку (manifests.js), не только в меню.
  assertFileRepairGame(gameId) {
    if (manifestRegistry.isFileRepairGame(gameId)) return;

    throw new DeliveryError(
      "REPAIR_SOURCE_MISSING",
      `Проверка и восстановление по файлам для ${gameId} пока недоступны`,
      { gameId, reason: "NOT_ENABLED" }
    );
  }

  async verifyFiles(gameId) {
    this.assertNotDepotBuild(gameId, "проверка файлов");
    this.assertFileRepairGame(gameId);

    const slot = this.reserveSlot(gameId, "verify");

    try {
      return await this.runVerifyFiles(gameId);
    } finally {
      this.releaseSlot(slot);
    }
  }

  async runVerifyFiles(gameId) {
    this.assertGameNotRunning(gameId);

    const installed = this.installedOf(gameId);
    const manifest = this.manifestFor(gameId).manifest;

    const files = await this.resolveFilesManifest(gameId, installed.version);

    this.activeGameId = gameId;
    this.setOperation(gameId, "verify");
    this.setStatus(gameId, { state: "checking-files", percent: 0, error: null });

    try {
      const result = await this.integrityService.check({
        gameId,
        installPath: installed.installPath,
        filesManifest: files,
        preservePaths: manifest.preservePaths,
        stockConfigs: true,
      });

      this.lastIntegrity.set(gameId, result);

      this.setStatus(gameId, {
        state: result.ok ? "files-valid" : "files-damaged",
        percent: 100,
        integrity: this.summarizeIntegrity(result),
      });

      return result;
    } catch (error) {
      const delivery = toDeliveryError(error, "VERIFY_FAILED");
      this.setStatus(gameId, { state: "error", error: delivery.toPublic() });
      throw delivery;
    } finally {
      this.activeGameId = null;
      this.setOperation(gameId, null);
    }
  }

  summarizeIntegrity(result) {
    return {
      total: result.total,
      checked: result.checked,
      valid: result.valid.length,
      missing: result.missing.length,
      corrupted: result.corrupted.length,
      sizeMismatch: result.sizeMismatch.length,
      configMissing: result.configMissing.length,
      configChanged: result.configChanged.length,
      stockMissing: result.stockMissing || 0,
      extra: result.extra.length,
      damaged: result.damaged,
      ok: result.ok,
      samples: this.damagedPathsOf(result).slice(0, 10),
    };
  }

  async repair(gameId, options) {
    this.assertNotDepotBuild(gameId, "восстановление файлов");
    this.assertFileRepairGame(gameId);

    const slot = this.reserveSlot(gameId, "repair");

    try {
      return await this.runRepair(gameId, options);
    } finally {
      this.releaseSlot(slot);
    }
  }

  // Файлы для ремонта берутся только из пофайлового зеркала версии: полный
  // архив ради пары файлов не качается никогда.
  async runRepair(gameId, options) {
    const opts = options || {};

    this.assertGameNotRunning(gameId);

    await this.assertSteamRequirements(gameId);

    const installed = this.installedOf(gameId);
    const manifest = this.manifestFor(gameId).manifest;
    const files = await this.resolveFilesManifest(gameId, installed.version);
    const mirror = await this.repairMirrorOf(gameId, installed.version);

    let integrity = this.lastIntegrity.get(gameId);
    if (!integrity || opts.recheck !== false) integrity = await this.runVerifyFiles(gameId);

    const offered = this.damagedPathsOf(integrity).concat(integrity.configChanged || []);

    if (!offered.length) {
      this.setStatus(gameId, { state: "files-valid", integrity: this.summarizeIntegrity(integrity) });
      return { status: "no-damage", repaired: [] };
    }

    // Стоковые cfg из папок игрока (files-manifest.js) — тоже из подписанного списка версии.
    const byPath = new Map(files.files.concat(files.stockConfigs || []).map((file) => [file.path, file]));

    if (!opts.confirmed)
      return {
        status: "confirmation-required",
        damaged: integrity.damaged,
        files: this.repairChoices(integrity, byPath),
      };

    // Отмеченные игроком; без выбора — всё повреждённое, изменённые cfg не трогаем.
    const chosen = new Set(Array.isArray(opts.files) ? opts.files : this.damagedPathsOf(integrity));
    const entries = offered.filter((item) => chosen.has(item)).map((item) => byPath.get(item)).filter(Boolean);

    if (!entries.length) return { status: "nothing-selected", repaired: [] };

    const plan = repairMirror.plan(mirror, entries, {
      version: installed.version,
      allowedProviderTypes: featureFlag.allowedProviderTypes(this.env),
    });

    if (!plan.usable) throw this.mirrorUnavailable(gameId, installed.version, plan);

    this.activeGameId = gameId;
    let root = null;

    try {
      root = await this.fetchFromMirror(gameId, mirror, plan, entries);

      this.setOperation(gameId, "repair");
      this.setStatus(gameId, { state: "repairing", percent: 0, error: null });

      const result = await this.repairService.repair({
        gameId,
        installPath: installed.installPath,
        filesManifest: files,
        preservePaths: manifest.preservePaths,
        preparedRoot: root,
        damagedFiles: entries.map((entry) => entry.path),
      });

      const recheck = await this.integrityService.check({
        gameId,
        installPath: installed.installPath,
        filesManifest: files,
        preservePaths: manifest.preservePaths,
        stockConfigs: true,
      });

      this.lastIntegrity.set(gameId, recheck);

      this.setStatus(gameId, {
        state: recheck.ok ? "repair-complete" : "files-damaged",
        percent: 100,
        integrity: this.summarizeIntegrity(recheck),
      });

      // «Осталось» — только файлы сборки; стоковые cfg, с которых сняли галочку,
      // интерфейс покажет мягко по integrity (stockMissing, configChanged).
      const stockPaths = new Set((files.stockConfigs || []).map((file) => file.path));
      return Object.assign({}, result, {
        integrity: this.summarizeIntegrity(recheck),
        remaining: this.damagedPathsOf(recheck).filter((relative) => !stockPaths.has(relative)),
      });
    } catch (error) {
      const delivery = toDeliveryError(error, "REPAIR_FAILED");
      // Отмена загрузки ничего не меняла в игре: повреждения те же.
      if (delivery.code === "CANCELLED")
        this.setStatus(gameId, {
          state: integrity.ok ? "files-valid" : "files-damaged",
          percent: 100,
          integrity: this.summarizeIntegrity(integrity),
          error: null,
        });
      else this.setStatus(gameId, { state: "error", error: delivery.toPublic() });
      throw delivery;
    } finally {
      if (root) this.discardMirrorSource(root);

      this.activeGameId = null;
      this.setOperation(gameId, null);
    }
  }

  repairChoices(integrity, byPath) {
    const kinds = [
      [integrity.missing, "missing"],
      [integrity.corrupted.map((item) => item.path), "damaged"],
      [integrity.sizeMismatch.map((item) => item.path), "damaged"],
      [integrity.configMissing || [], "cfg-missing"],
      [integrity.configChanged || [], "cfg-changed"],
    ];

    const choices = [];
    for (const [list, kind] of kinds)
      for (const item of list) if (byPath.has(item)) choices.push({ path: item, size: byPath.get(item).size, kind });

    return choices;
  }

  damagedPathsOf(integrity) {
    return integrity.missing
      .concat(integrity.corrupted.map((item) => item.path))
      .concat(integrity.sizeMismatch.map((item) => item.path))
      .concat(integrity.configMissing || []);
  }

  repairMirrorFor(gameId, version) {
    const last = this.updateChecker.lastResult(gameId);
    const target = last && last.target ? last.target : null;
    const mirror = target ? target.repairMirror : null;

    return mirror && mirror.version === version ? mirror : null;
  }

  // Зеркало объявляет подписанный канал версии; без него ремонт не начинается.
  async repairMirrorOf(gameId, version) {
    if (!this.updateChecker.lastResult(gameId)) await this.updateChecker.check(gameId);

    const mirror = this.repairMirrorFor(gameId, version);
    const reason = repairMirror.unavailableReason(mirror, {
      version,
      allowedProviderTypes: featureFlag.allowedProviderTypes(this.env),
    });

    if (reason) throw this.mirrorUnavailable(gameId, version, { reason });
    return mirror;
  }

  mirrorUnavailable(gameId, version, plan) {
    const file = plan.oversized ? plan.oversized[0] : null;
    const last = this.updateChecker.lastResult(gameId);
    // Канал описывает новую версию: обновление заменит и повреждённые файлы.
    const update = !file && !!(last && last.updateAvailable);

    this.logger.warn(`[game-delivery] ремонт ${gameId} ${version} по файлам недоступен: ${plan.reason}`);

    return new DeliveryError(
      "REPAIR_SOURCE_MISSING",
      file
        ? `Файл ${file} недоступен в зеркале ремонта`
        : update
          ? `Восстановление по файлам для версии ${version} недоступно — сначала обнови игру`
          : `Восстановление по файлам для версии ${version} пока недоступно — переустанови игру`,
      { gameId, version, reason: update ? "UPDATE_AVAILABLE" : plan.reason, path: file }
    );
  }

  async fetchFromMirror(gameId, mirror, plan, entries) {
    const root = assertInside(
      this.paths.staging,
      path.join(this.paths.staging, `repair-mirror-${gameId}-${Date.now().toString(36)}`),
      "staging зеркала ремонта"
    );
    const controller = new AbortController();

    this.repairAbort = controller;
    this.setOperation(gameId, "repair-download");
    this.setStatus(gameId, {
      state: "preparing",
      percent: 0,
      bytesDownloaded: 0,
      totalBytes: plan.totalBytes,
      error: null,
    });

    try {
      const result = await this.repairMirror.fetchInto(root, entries, {
        providers: ManifestService.orderProviders(plan.providers),
        layout: mirror.layout,
        signal: controller.signal,
        onProgress: (progress) =>
          this.setStatus(gameId, {
            state: "preparing",
            percent: progress.percent,
            bytesDownloaded: progress.bytes,
            totalBytes: progress.totalBytes,
          }),
      });

      // Крестик нажат, пока докачивался последний файл: в игру ничего не пишем.
      if (controller.signal.aborted) throw new DeliveryError("CANCELLED", "Восстановление отменено");

      this.logger.info(
        `[game-delivery] с зеркала ремонта получено файлов: ${result.files.length}, ` +
          `${result.bytes} B (${gameId})`
      );

      return root;
    } catch (error) {
      this.discardMirrorSource(root);
      throw error;
    } finally {
      this.repairAbort = null;
    }
  }

  discardMirrorSource(root) {
    try {
      if (root && fs.existsSync(root)) removeWithinZone(this.paths.staging, root);
    } catch (error) {
      this.logger.warn(`[game-delivery] не удалось убрать ${root}: ${error.message}`);
    }
  }

  targetFor(gameId) {
    const last = this.updateChecker.lastResult(gameId);
    if (last && last.target) return last.target;

    const loaded = this.manifestFor(gameId);
    const target = updateTarget.fromLocal(loaded.manifest);

    if (target) target.manifestSha256 = loaded.sha256;
    return target;
  }

  planUpdate(gameId) {
    const target = this.targetFor(gameId);
    const installed = this.installService.readInstalled(gameId);

    if (!target)
      return { kind: "up-to-date", steps: [], totalSize: 0, reason: "no-target" };

    return patchChain.planUpdate({
      patches: target.patches || [],
      fromVersion: installed ? installed.version : null,
      toVersion: target.version,
      fullSize: target.fullArchive ? target.fullArchive.size : 0,
    });
  }

  async checkForUpdates(gameId, options) {
    const opts = options || {};

    this.assertNotDepotBuild(gameId, "проверка обновлений");

    const result = await this.updateChecker.check(gameId, {
      force: opts.force !== false,
      reason: opts.reason || "manual",
    });

    const plan = this.planUpdate(gameId);

    const status = this.setStatus(gameId, {
      updatePlan: {
        kind: plan.kind,
        reason: plan.reason || null,
        steps: plan.steps.map((step) => `${step.fromVersion} -> ${step.toVersion}`),
        totalSize: plan.totalSize,
      },
    });

    return { status, plan, update: this.publicUpdate(gameId), raw: result };
  }

  checkForUpdatesIfDue(gameId, reason) {
    if (manifestRegistry.isDepotBuild(gameId)) return Promise.resolve(null);

    return this.updateChecker
      .check(gameId, { force: false, reason: reason || "scheduled" })
      .catch((error) => {
        this.logger.warn(
          `[game-delivery] фоновая проверка обновлений ${gameId}: ${error.message}`
        );
        return null;
      });
  }

  checkForUpdatesOnFocus(gameId) {
    if (manifestRegistry.isDepotBuild(gameId)) return Promise.resolve(null);

    const FOCUS_CHECK_AGE_MS = 5 * 60 * 1000;
    return this.updateChecker
      .checkIfOlderThan(gameId, FOCUS_CHECK_AGE_MS, "focus")
      .catch((error) => {
        this.logger.warn(
          `[game-delivery] focus-проверка обновлений ${gameId}: ${error.message}`
        );
        return null;
      });
  }

  async applyUpdate(gameId) {
    const slot = this.reserveSlot(gameId, "update");

    try {
      return await this.runApplyUpdate(gameId);
    } finally {
      this.releaseSlot(slot);
    }
  }

  async runApplyUpdate(gameId) {
    this.assertGameNotRunning(gameId);

    await this.assertSteamRequirements(gameId);

    const loaded = this.manifestFor(gameId);
    const manifest = loaded.manifest;
    const plan = this.planUpdate(gameId);

    if (plan.kind === "up-to-date") return { status: "up-to-date" };

    if (plan.kind === "full" && plan.reason === "downgrade")
      return { status: "up-to-date", reason: "downgrade" };

    if (plan.kind === "full") return this.applyFullUpdate(gameId, plan);

    ensureLayout(this.paths);

    const allowedProviderTypes = featureFlag.allowedProviderTypes(this.env);
    this.activeGameId = gameId;
    this.setOperation(gameId, "update");

    const chainTotalBytes = plan.steps.reduce(
      (sum, step) => sum + (step.archive ? step.archive.size : 0),
      0
    );
    let chainDoneBytes = 0;

    const targetVersion = plan.steps[plan.steps.length - 1].toVersion;
    await this.tryEnsureFilesManifest(gameId, targetVersion);

    const versionBeforeUpdate = this.installedVersionOf(gameId);

    try {
      for (let index = 0; index < plan.steps.length; index++) {
        const step = plan.steps[index];
        const installed = this.installedOf(gameId);

        this.setStatus(gameId, {
          chain: {
            step: index + 1,
            steps: plan.steps.length,
            fromVersion: step.fromVersion,
            toVersion: step.toVersion,
            targetVersion: plan.steps[plan.steps.length - 1].toVersion,
            doneBytes: chainDoneBytes,
            totalBytes: chainTotalBytes,
          },
        });

        const usable = step.archive.providers.filter(
          (provider) => allowedProviderTypes.indexOf(provider.type) !== -1
        );

        if (!usable.length)
          throw new DeliveryError("REMOTE_DISABLED", "Источник патча выключен feature flag'ом");

        this.setStatus(gameId, { state: "preparing", percent: 0, error: null });

        const download = await this.downloadWithFallback({
          gameId,
          descriptors: usable,
          filename: step.archive.filename,
          expectedSize: step.archive.size,
          expectedSha256: step.archive.sha256,
          disabledMessage: "Источник патча выключен feature flag'ом",
        });

        if (download.status !== "completed") {
          this.refresh(gameId);
          return download;
        }

        const patchManifest = await this.loadPatchManifest(
          gameId,
          step,
          download.filePath,
          usable
        );

        try {
          await this.patchService.apply({
            gameManifest: (this.buildDescriptorFor(gameId, step.toVersion) || {}).manifest || manifest,
            patchManifest,
            installedVersion: installed.version,
            archivePath: download.filePath,
          });
        } catch (error) {
          // Манифест мог быть подменён (files[]): повтор скачает его с зеркала заново.
          this.dropPatchManifest(download.filePath);
          throw error;
        }

        this.removeArchive(gameId, step.archive.filename);
        chainDoneBytes += step.archive.size;
      }

      this.updateChecker.invalidate(gameId);
      await this.updateChecker.check(gameId, { force: true, reason: "after-update" });

      const status = this.refresh(gameId);
      const target = this.targetFor(gameId);

      this.announceUpdated(gameId, versionBeforeUpdate, targetVersion);

      return {
        status: "updated",
        version: target ? target.version : manifest.version,
        delivery: status,
      };
    } catch (error) {
      const delivery = toDeliveryError(error, "PATCH_FAILED");
      this.setStatus(gameId, { state: "error", error: delivery.toPublic() });
      throw delivery;
    } finally {
      this.activeGameId = null;

      if (this.statusOf(gameId).state !== "paused") {
        this.setOperation(gameId, null);
        this.setStatus(gameId, { chain: null });
      } else {
        this.setStatus(gameId, {});
      }
    }
  }

  async applyFullUpdate(gameId, plan) {
    const installed = this.installService.readInstalled(gameId);

    if (!installed) return this.installToLatest(gameId);

    const loaded = this.manifestFor(gameId);
    const target = this.targetFor(gameId);
    const descriptor = target ? target.fullArchive : null;

    if (!descriptor)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        "Источник не объявил полный архив целевой версии — обновляться нечем",
        { gameId, reason: plan.reason || null }
      );

    const targetVersion = target.version;

    const archiveVersion = descriptor.version || targetVersion;

    if (archiveVersion !== targetVersion)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        `Полный архив содержит версию ${archiveVersion}, а обновление ведёт к ${targetVersion}: ` +
          `нужен патч ${archiveVersion} -> ${targetVersion}, но канал его не объявил`,
        { gameId, archiveVersion, targetVersion }
      );

    const allowedProviderTypes = featureFlag.allowedProviderTypes(this.env);
    const usable = (descriptor.providers || []).filter(
      (provider) => allowedProviderTypes.indexOf(provider.type) !== -1
    );

    ensureLayout(this.paths);

    this.activeGameId = gameId;
    this.setOperation(gameId, "update");

    try {
      this.setStatus(gameId, {
        state: "preparing",
        percent: 0,
        error: null,
        chain: {
          step: 1,
          steps: 1,
          fromVersion: installed.version,
          toVersion: targetVersion,
          targetVersion,
          doneBytes: 0,
          totalBytes: descriptor.size,
        },
      });

      // Полный архив ставится только с SHA-256 из подписанного канала или
      // встроенного манифеста: сборку без закреплённого хэша не ставим.
      if (!descriptor.sha256)
        throw new DeliveryError(
          "CHECKSUM_MISMATCH",
          "У полного архива нет закреплённого SHA-256 — такая сборка не ставится",
          { gameId, version: targetVersion }
        );

      const files = await this.resolveFilesManifest(gameId, targetVersion);

      const localArchive = this.findLocalArchive(gameId, descriptor);
      this.checkFullUpdateSpace(
        descriptor,
        files || { totalSize: loaded.manifest.expectedInstalledSize },
        {
          archiveLocal: !!localArchive,
          userDataBytes: this.installService.userDataBytes({ gameId, installPath: gameInstallPath(this.paths, gameId), manifest: loaded.manifest }),
        }
      );

      let archivePath = localArchive;

      if (!archivePath) {
        const download = await this.downloadWithFallback({
          gameId,
          descriptors: usable,
          filename: descriptor.filename,
          expectedSize: descriptor.size,
          expectedSha256: descriptor.sha256,
          disabledMessage: "Источник полного архива выключен feature flag'ом",
        });

        if (download.status !== "completed") {
          this.refresh(gameId);
          return download;
        }

        archivePath = download.filePath;

        this.markArchiveOrigin(gameId, descriptor);
      }

      const result = await this.installService.install({
        manifest: this.fullUpdateManifest(loaded.manifest, target, descriptor, files),
        manifestSha256: target.manifestSha256 || loaded.sha256,
        archivePath,
        update: {
          fromVersion: installed.version,
          filesManifest: files,
          integrityService: this.integrityService,
        },
      });

      this.cleanupFullUpdateArchive(gameId, descriptor);

      this.updateChecker.invalidate(gameId);
      await this.updateChecker.check(gameId, { force: true, reason: "after-update" });

      const status = this.refresh(gameId);

      this.announceUpdated(gameId, installed.version, result.version);

      return { status: "updated", version: result.version, kind: "full", delivery: status };
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");
      this.setStatus(gameId, {
        state: delivery.code === "CANCELLED" ? "cancelled" : "error",
        error: delivery.toPublic(),
      });
      throw delivery;
    } finally {
      this.activeGameId = null;

      if (this.statusOf(gameId).state !== "paused") {
        this.setOperation(gameId, null);
        this.setStatus(gameId, { chain: null });
      } else {
        this.setStatus(gameId, {});
      }
    }
  }

  fullUpdateManifest(localManifest, target, descriptor, files) {
    return Object.assign({}, localManifest, {
      version: target.version,
      publishedAt: target.publishedAt || null,
      expectedInstalledSize: target.expectedInstalledSize || files.totalSize,
      buildMetadata:
        target.buildMetadata ||
        (localManifest.version === target.version ? localManifest.buildMetadata : null) ||
        null,
      baseArchive: {
        filename: descriptor.filename,
        size: descriptor.size,
        sha256: descriptor.sha256,
        rootPrefix: descriptor.rootPrefix || null,
        providers: descriptor.providers || [],
      },
    });
  }

  findLocalArchive(gameId, descriptor) {
    const candidate = path.join(this.paths.downloads, gameId, path.basename(descriptor.filename));

    let stats;
    try {
      stats = fs.statSync(candidate);
    } catch (e) {
      return null;
    }

    if (!stats.isFile() || stats.size !== descriptor.size) return null;

    const digest = filesManifestLib.hashFileSync(candidate);

    if (digest !== descriptor.sha256) {
      this.logger.warn(
        `[game-delivery] архив в Downloads не подходит для версии: ` +
          `SHA-256 ${digest} вместо ${descriptor.sha256} — будет скачан заново`
      );
      return null;
    }

    return candidate;
  }

  checkFullUpdateSpace(descriptor, files, options) {
    const opts = options || {};

    if (typeof fs.statfsSync !== "function") return;

    let free = null;
    try {
      const stat = fs.statfsSync(this.paths.root);
      free = stat.bavail * stat.bsize;
    } catch (e) {
      return;
    }

    const margin = 512 * 1024 * 1024;
    // userDataBytes — копии файлов игрока в новой сборке (как InstallService.checkSpace).
    const required = (opts.archiveLocal ? 0 : descriptor.size) + (files.totalSize || 0) + (opts.userDataBytes || 0) + margin;

    if (free < required)
      throw new DeliveryError(
        "NO_SPACE",
        `Недостаточно места для обновления: нужно ${required}, свободно ${free}`,
        { requiredBytes: required, freeBytes: free }
      );
  }

  archiveOriginPath(gameId) {
    return path.join(this.paths.state, `${gameId}.archive-origin.json`);
  }

  markArchiveOrigin(gameId, descriptor) {
    try {
      const { writeJsonAtomic } = require("./atomic");
      writeJsonAtomic(this.archiveOriginPath(gameId), {
        filename: path.basename(descriptor.filename),
        sha256: descriptor.sha256,
        downloadedByLauncher: true,
        at: new Date().toISOString(),
      });
    } catch (e) {
      this.logger.warn(`[game-delivery] пометка происхождения архива не записана: ${e.message}`);
    }
  }

  readArchiveOrigin(gameId) {
    const { readJsonSafe } = require("./atomic");
    return readJsonSafe(this.archiveOriginPath(gameId));
  }

  clearArchiveOrigin(gameId) {
    try {
      fs.rmSync(this.archiveOriginPath(gameId), { force: true });
    } catch (e) {
      this.logger.warn(`[game-delivery] пометка происхождения архива не удалена: ${e.message}`);
    }
  }

  cleanupFullUpdateArchive(gameId, descriptor) {
    const origin = this.readArchiveOrigin(gameId);

    const ours =
      origin &&
      origin.downloadedByLauncher &&
      origin.filename === path.basename(descriptor.filename) &&
      origin.sha256 === descriptor.sha256;

    if (!ours) {
      this.logger.info(
        `[game-delivery] архив ${descriptor.filename} предоставлен не лаунчером — оставляю`
      );
      return;
    }

    this.downloadManager.discard(gameId, descriptor.filename);
    this.removeArchive(gameId, descriptor.filename);
    this.clearArchiveOrigin(gameId);
  }

  pendingFullUpdate(gameId) {
    return this.installService.pendingFullUpdate(gameId);
  }

  cleanupOrphanStaging() {
    return this.installService.cleanupOrphanStaging();
  }

  recoverFullUpdate(gameId) {
    const pending = this.installService.pendingFullUpdate(gameId);
    const result = this.installService.recoverFullUpdate(gameId);

    if (result && result.recovered && result.direction === "forward" && pending) {
      const origin = this.readArchiveOrigin(gameId);

      if (origin && origin.downloadedByLauncher && origin.filename) {
        this.downloadManager.discard(gameId, origin.filename);
        this.removeArchive(gameId, origin.filename);
        this.clearArchiveOrigin(gameId);
      }

      this.updateChecker.invalidate(gameId);
    }

    return result;
  }

  async loadPatchManifest(gameId, step, archivePath, descriptors) {
    const fs = require("fs");
    const path = require("path");
    const httpClient = require("./http-client");

    const candidate = archivePath.replace(/\.7z$/i, ".json");

    if (!fs.existsSync(candidate)) {
      const allowedProviderTypes = featureFlag.allowedProviderTypes(this.env);
      const usable = (descriptors || []).filter(
        (entry) => allowedProviderTypes.indexOf(entry.type) !== -1
      );

      if (!usable.length)
        throw new DeliveryError("MANIFEST_NOT_FOUND", "Нет манифеста патча и нет источника");

      const outcome = await this.mirrors.run({
        artifact: `${gameId}:patch-manifest:${path.basename(candidate)}`,
        descriptors: ManifestService.orderProviders(usable),
        operation: async ({ descriptor: entry }) => {
          const provider = providers.create(entry, { env: this.env, allowedProviderTypes });
          const sibling = await provider.resolveSibling(path.basename(candidate));
          const response = await httpClient.request(sibling.url, { headers: sibling.headers });

          if (response.statusCode !== 200) {
            response.resume();
            throw new DeliveryError(
              "MANIFEST_NOT_FOUND",
              `Манифест патча недоступен: HTTP ${response.statusCode}`,
              { status: response.statusCode, retryAfter: response.headers["retry-after"] || null }
            );
          }

          return httpClient.readBody(response, 8 * 1024 * 1024);
        },
      });

      this.lastMirrorAttempts = outcome.attempts;
      fs.writeFileSync(candidate, outcome.value);
    }

    try {
      let data;
      try {
        data = JSON.parse(fs.readFileSync(candidate, "utf8"));
      } catch (e) {
        throw new DeliveryError("MANIFEST_INVALID", `Манифест патча не читается: ${e.message}`);
      }

      return this.checkPatchManifest(gameId, step, data);
    } catch (error) {
      // Отвергнутый манифест не остаётся в Downloads: повтор скачает его заново.
      this.dropPatchManifest(archivePath);
      throw error;
    }
  }

  // Ошибка удаления не должна подменять настоящую ошибку патча.
  dropPatchManifest(archivePath) {
    try {
      fs.rmSync(archivePath.replace(/\.7z$/i, ".json"), { force: true });
    } catch (error) {
      this.logger.warn(`[game-delivery] отвергнутый манифест патча не удалён: ${error.message}`);
    }
  }

  // Манифест патча приходит с зеркала без подписи. files[] привязаны к
  // закреплённому архиву, остальное сверяется с подписанным шагом.
  checkPatchManifest(gameId, step, data) {
    const patchManifest = patchFormat.validatePatchManifest(data, {
      expectedFrom: step.fromVersion,
    });

    if (patchManifest.toVersion !== step.toVersion)
      throw new DeliveryError("MANIFEST_INVALID", "Версия патча не совпадает с манифестом игры");

    if (patchManifest.archive.sha256 !== step.archive.sha256)
      throw new DeliveryError("CHECKSUM_MISMATCH", "SHA-256 патча не совпадает с манифестом игры");

    if (patchManifest.resultingFilesManifestSha256 !== step.resultingFilesManifestSha256)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        "Ожидаемый файловый манифест патча не совпадает с манифестом игры"
      );

    // deletedFiles ничем не подписаны. Удалять можно только то, что прежняя
    // версия поставляла, а итоговая (её список закреплён подписанным
    // resultingFilesManifestSha256) уже не содержит: сейвы и демки ни в один
    // файловый манифест не входят.
    const filesOf = (version) => {
      try {
        return this.filesManifestFor(gameId, version);
      } catch (error) {
        return null;
      }
    };
    const previous = filesOf(step.fromVersion);
    let resulting = filesOf(step.toVersion);
    if (resulting && filesManifestLib.hashManifest(resulting) !== step.resultingFilesManifestSha256)
      resulting = null;

    let deletedFiles = patchManifest.deletedFiles;

    if (!previous || !resulting) {
      if (deletedFiles.length)
        this.logger.warn(
          `[game-delivery] ${gameId} ${step.fromVersion} -> ${step.toVersion}: нет подписанных файловых ` +
            `манифестов обеих версий — удаления из манифеста патча (${deletedFiles.length}) пропущены`
        );
      deletedFiles = [];
    } else {
      const shipped = new Set(previous.files.map((file) => file.path));
      const kept = new Set(resulting.files.map((file) => file.path));
      const foreign = deletedFiles.filter((relative) => !shipped.has(relative) || kept.has(relative));

      if (foreign.length)
        throw new DeliveryError(
          "MANIFEST_INVALID",
          `Манифест патча удаляет файлы не из прежней сборки: ${foreign.slice(0, 5).join(", ")}`,
          { files: foreign.slice(0, 20) }
        );
    }

    // Как build-patch (patch-format.diff): пути игрока из preservePaths патч не удаляет.
    const preserved = createMatcher(this.manifestFor(gameId).manifest.preservePaths || []);
    deletedFiles = deletedFiles.filter((relative) => !preserved(relative));

    if (deletedFiles.length === patchManifest.deletedFiles.length) return patchManifest;

    // Самохеш покрывает deletedFiles: без пересчёта PatchService.apply отвергнет манифест.
    // Сам хеш ничего не закрепляет, файлы патча проверяются по архиву и своим SHA-256.
    const trimmed = Object.assign({}, patchManifest, { deletedFiles });
    if (trimmed.patchManifestSha256) trimmed.patchManifestSha256 = patchFormat.hashPatchManifest(trimmed);
    return trimmed;
  }

  recoverPatch(gameId) {
    return this.patchService.recover(gameId);
  }

  pendingPatch(gameId) {
    return this.patchService.findPending(gameId);
  }

  recoverRepair(gameId) {
    return this.repairService.recover(gameId);
  }

  pendingRepair(gameId) {
    return this.repairService.findPending(gameId);
  }

  pause() {
    return this.downloadManager.pause();
  }

  cancel(gameId, options) {
    const activeId = (this.slot && this.slot.gameId) || this.activeGameId;
    if (gameId && this.statusOf(gameId).state === "paused" && activeId !== gameId) {
      this.downloadManager.discardPaused(gameId, (fileName) => !!episodeInstall.episodeOfArchive(fileName));
      this.setOperation(gameId, null);
      this.setStatus(gameId, { state: "cancelled", percent: 0, bytesDownloaded: 0, chain: null, error: null });
      this.refresh(gameId);
      return true;
    }
    // Эпизод на паузе или его архив, оставленный отказом: Ghosting в «ready», а файлы лежат — отмена их убирает.
    if (gameId === episodeInstall.GAME_ID && activeId !== gameId) {
      // { episode } из карточки эпизода — только его архив; чужое значение или без него — архивы всех.
      const episode = options && episodes.isEpisode(options.episode) ? options.episode : null;
      let removed = 0;
      for (const id of episode ? [episode] : Object.keys(episodes.EPISODES)) removed += this.discardStaleEpisodeArchives(id, null);
      if (removed) {
        this.refresh(gameId);
        return true;
      }
    }
    if (gameId && activeId && gameId !== activeId) return false;
    if (this.packCancel) {
      this.packCancel();
      return true;
    }
    if (this.repairAbort) {
      this.repairAbort.abort();
      return true;
    }
    return this.downloadManager.cancel() || this.installService.cancel();
  }

  listPendingOperations() {
    return this.installService.listPendingOperations();
  }

  cleanupPending(gameId) {
    const result = this.installService.cleanupPending(gameId);
    this.refresh(gameId);
    return result;
  }

  async uninstallGame(gameId, options) {
    const slot = this.reserveSlot(gameId, "uninstall");

    try {
      return await this.runUninstallGame(gameId, options);
    } finally {
      this.releaseSlot(slot);
    }
  }

  async runUninstallGame(gameId, options) {
    const opts = options || {};

    this.assertGameNotRunning(gameId);

    const plan = gameRemoval.planRemoval(this.paths, gameId);

    if (!opts.confirmed)
      return {
        status: "confirmation-required",
        gameId: plan.gameId,
        installPath: plan.installPath,
        version: this.installedVersionOf(plan.gameId),
        totalBytes: plan.totalBytes,
        targets: plan.targets.map((target) => ({ kind: target.kind, bytes: target.bytes })),
      };

    this.activeGameId = plan.gameId;
    this.setOperation(plan.gameId, "uninstall");

    let result;
    try {
      result = gameRemoval.removeGame(this.paths, plan.gameId);
    } finally {
      this.activeGameId = null;
      this.setOperation(plan.gameId, null);
    }

    this.updateChecker.invalidate(plan.gameId);
    this.lastIntegrity.delete(plan.gameId);

    if (result.partial)
      this.logger.warn(
        `[game-delivery] удаление ${plan.gameId}: не удалось удалить ` +
          `${result.failed.length} путей, первый — ${result.failed[0].path}`
      );
    else
      this.logger.info(
        `[game-delivery] игра ${plan.gameId} удалена, освобождено ${plan.totalBytes} байт`
      );

    this.setStatus(plan.gameId, {
      state: "not-installed",
      percent: 0,
      error: null,
      integrity: null,
      chain: null,
    });

    const status = this.refresh(plan.gameId);

    return {
      status: result.partial ? "partial" : "removed",
      gameId: plan.gameId,
      freedBytes: result.freedBytes,
      removed: result.removed,
      leftovers: result.failed.slice(0, 10).map((item) => item.path),
      installPath: plan.installPath,
      delivery: status,
    };
  }

  // Полная сборка в архив .hlsrpack (game-pack.js). Игра должна быть
  // закрыта: иначе в архив попали бы файлы посреди записи.
  async exportPack(gameId, targetPath, extra) {
    const slot = this.reserveSlot(gameId, "export");
    const work = assertInside(
      this.paths.staging,
      path.join(this.paths.staging, `${gameId}-export-${crypto.randomBytes(4).toString("hex")}`),
      "Папка экспорта"
    );
    const partial = `${targetPath}.hlsr-partial`;

    try {
      this.assertGameNotRunning(gameId);
      if (manifestRegistry.isDepotBuild(gameId))
        throw new DeliveryError("PACK_DEPOT_BUILD", "Сборку из Steam целиком не перенести");
      const installed = this.installService.readInstalled(gameId);
      if (!installed) throw new DeliveryError("NOT_INSTALLED", "Сборка не установлена", { gameId });

      // Изменённые файлы сборки (кастомный HUD поверх файлов игры) едут как есть,
      // но сперва игрок узнаёт, что при импорте они переедут (restorePackOriginals).
      if (!(extra && extra.allowModified)) {
        const modified = await this.packModifiedCount(gameId, installed);
        if (modified)
          throw new DeliveryError("PACK_MODIFIED", `В сборке изменено файлов: ${modified}`, {
            modified,
            custom: !!gamePack.customLayerOf(gameId),
          });
      }

      const { files, bytes } = gamePack.collect(installed.installPath);
      fs.mkdirSync(work, { recursive: true });
      const metaFile = path.join(work, gamePack.META_FILE);
      fs.writeFileSync(
        metaFile,
        JSON.stringify(gamePack.meta(Object.assign({}, extra, { installed, files: files.length, bytes })), null, 1),
        "utf8"
      );

      this.activeGameId = gameId;
      this.setOperation(gameId, "export");
      this.setStatus(gameId, { state: "exporting", percent: 0, bytesDownloaded: 0, totalBytes: bytes, chain: null, error: null });

      fs.rmSync(partial, { force: true });
      const job = this.archiveService.create(partial, {
        cwd: installed.installPath,
        files,
        headFiles: [metaFile],
        listDir: work,
        threads: require("os").cpus().length,
        onProgress: (percent) =>
          this.setStatus(gameId, { percent, bytesDownloaded: Math.round((bytes * percent) / 100) }),
      });
      this.packCancel = job.cancel;
      await job.promise;

      fs.rmSync(targetPath, { force: true });
      fs.renameSync(partial, targetPath);
      const size = fs.statSync(targetPath).size;
      this.logger.info(`[game-delivery] ${gameId}: сборка сохранена в ${targetPath} (${files.length} файлов, ${size} байт)`);
      return { status: "ok", gameId, files: files.length, bytes, size, path: targetPath };
    } catch (error) {
      try {
        fs.rmSync(partial, { force: true });
      } catch (cleanupError) {
        this.logger.warn(`[game-delivery] не удалось удалить ${partial}: ${cleanupError.message}`);
      }
      if (error && error.code === "CANCELLED") this.setStatus(gameId, { state: "cancelled", percent: 0, error: null });
      throw error instanceof gamePack.PackError ? new DeliveryError("PACK_" + error.code, error.message) : error;
    } finally {
      this.packCancel = null;
      try {
        if (fs.existsSync(work)) removeWithinZone(this.paths.staging, work);
      } catch (cleanupError) {
        this.logger.warn(`[game-delivery] не удалось удалить ${work}: ${cleanupError.message}`);
      }
      if (this.activeGameId === gameId) this.activeGameId = null;
      this.setOperation(gameId, null);
      this.releaseSlot(slot);
      this.refresh(gameId);
    }
  }

  // Сколько файлов сборки не совпало со списком версии. Это лишь предупреждение:
  // без списка версии (нет сети) экспорт идёт как есть.
  async packModifiedCount(gameId, installed) {
    let filesManifest = null;
    try {
      filesManifest = await this.resolveFilesManifest(gameId, installed.version);
    } catch (error) {
      filesManifest = null;
    }
    if (!filesManifest || !Array.isArray(filesManifest.files)) return 0;
    this.activeGameId = gameId;
    this.setOperation(gameId, "export");
    this.setStatus(gameId, { state: "checking-files", percent: 0, checked: 0, totalChecked: filesManifest.files.length, error: null });
    this.packCancel = () => this.integrityService.cancel();
    const check = await this.integrityService.check({
      gameId,
      installPath: installed.installPath,
      filesManifest,
      preservePaths: this.manifestFor(gameId).manifest.preservePaths || [],
    });
    this.packCancel = null;
    return check.damaged;
  }

  // Сведения из архива .hlsrpack — до вопроса о замене установленной сборки.
  async readPackMeta(archivePath) {
    let text;
    try {
      text = await this.archiveService.readText(archivePath, gamePack.META_FILE, 4 * 1024 * 1024);
    } catch (error) {
      if (error && error.code === "ANTIVIRUS_BLOCKED") throw error;
      throw new DeliveryError("PACK_BAD_FORMAT", `Не архив сборки HLSR: ${error.message}`);
    }
    let meta;
    try {
      meta = gamePack.parseMeta(text, {
        isKnownGame: (gameId) => manifestRegistry.isKnownGame(gameId),
        isDepotBuild: (gameId) => manifestRegistry.isDepotBuild(gameId),
      });
    } catch (error) {
      throw new DeliveryError("PACK_" + (error.code || "BAD_FORMAT"), error.message);
    }
    return Object.assign(meta, { installedVersion: this.installedVersionOf(meta.gameId) });
  }

  // Файлы сборки в архиве не сошлись со списком версии — чаще всего кастомный
  // HUD поверх файлов игры. Импорт не отменяется: изменённый контент
  // откладывается, изменённые программы отбрасываются (gamePack.setAside), а
  // оригиналы всех — с зеркала ремонта, с проверкой размера и SHA-256. Нет сети
  // или зеркала этой версии — PACK_ORIGINALS_UNAVAILABLE, установленная сборка
  // не тронута (staging просто удаляется).
  async restorePackOriginals(gameId, version, stagingPath, filesManifest, check) {
    const byPath = new Map(filesManifest.files.map((file) => [file.path, file]));
    const entries = this.damagedPathsOf(check).map((relative) => byPath.get(relative)).filter(Boolean);
    const changed = check.corrupted.map((item) => item.path).concat(check.sizeMismatch.map((item) => item.path));
    let root = null;
    try {
      const mirror = await this.repairMirrorOf(gameId, version);
      const plan = repairMirror.plan(mirror, entries, { version, allowedProviderTypes: featureFlag.allowedProviderTypes(this.env) });
      if (!plan.usable) throw this.mirrorUnavailable(gameId, version, plan);
      root = await this.fetchFromMirror(gameId, mirror, plan, entries);
    } catch (error) {
      const delivery = toDeliveryError(error, "PACK_ORIGINALS_UNAVAILABLE");
      if (["CANCELLED", "NO_SPACE", "PATH_NOT_WRITABLE", "ANTIVIRUS_BLOCKED"].includes(delivery.code)) throw delivery;
      throw new DeliveryError(
        "PACK_ORIGINALS_UNAVAILABLE",
        `В архиве изменено файлов сборки ${changed.length}, оригиналы версии ${version} не скачать: ${delivery.message}`,
        { changed: changed.length, cause: delivery.code, samples: changed.slice(0, 10) }
      );
    }
    try {
      this.setOperation(gameId, "import");
      const aside = gamePack.setAside(stagingPath, gameId, changed);
      for (const entry of entries)
        fileOps.copyFileSync(
          fileOps.resolveInside(root, entry.path, `Эталон "${entry.path}"`),
          fileOps.resolveInside(stagingPath, entry.path, `Файл "${entry.path}"`),
          entry.path
        );
      this.logger.warn(
        `[game-delivery] ${gameId}: в архиве изменены файлы сборки (${changed.slice(0, 10).join(", ")}): ` +
          `в custom ${aside.custom.length}, в копии ${aside.backup.length}, программ отброшено ${aside.rejected.length}, ` +
          `оригиналов скачано ${entries.length}`
      );
      return {
        changed: changed.length,
        restored: entries.length,
        restoredPaths: entries.map((entry) => entry.path),
        custom: aside.custom.length,
        backup: aside.backup.length,
        rejected: aside.rejected.length,
      };
    } finally {
      this.discardMirrorSource(root);
    }
  }

  // Установка сборки из архива. Файлы сборки сверяются с подписанным
  // списком версии; изменённые заменяются оригиналами с сервера
  // (restorePackOriginals); посторонние и неподтверждённые исполняемые файлы
  // отбрасываются (game-pack.js). Установленная сборка заменяется целиком,
  // только после проверки.
  async importPack(archivePath, meta) {
    const gameId = meta.gameId;
    const slot = this.reserveSlot(gameId, "import");
    const stagingPath = assertInside(
      this.paths.staging,
      path.join(this.paths.staging, `${gameId}-import-${crypto.randomBytes(4).toString("hex")}`),
      "Папка импорта"
    );
    let locked = false;

    try {
      this.assertGameNotRunning(gameId);
      const manifest = this.manifestFor(gameId).manifest;
      const filesManifest = await this.resolveFilesManifest(gameId, meta.version);
      if (!filesManifest || !Array.isArray(filesManifest.files))
        throw new DeliveryError("PACK_VERSION_UNKNOWN", `Нет списка файлов версии ${meta.version}`, {
          version: meta.version,
        });

      this.activeGameId = gameId;
      this.setOperation(gameId, "import");
      this.setStatus(gameId, { state: "importing", percent: 0, bytesDownloaded: 0, totalBytes: meta.bytes || 0, chain: null, error: null });

      this.installService.checkWritable();
      const inspection = await this.archiveService.inspect(archivePath, {
        maxEntries: gamePack.MAX_ENTRIES,
        maxUncompressedSize: gamePack.MAX_UNCOMPRESSED_BYTES,
      });
      const total = inspection.report.uncompressedSize || meta.bytes || 0;
      this.setStatus(gameId, { totalBytes: total });
      this.installService.checkSpace(inspection.report.uncompressedSize || 1);
      this.installService.acquireLock(gameId);
      locked = true;

      fs.mkdirSync(stagingPath, { recursive: true });
      const job = this.archiveService.extract(archivePath, stagingPath, {
        onProgress: (fraction) =>
          this.setStatus(gameId, { percent: Math.round(fraction * 100), bytesDownloaded: Math.round(total * fraction) }),
      });
      this.packCancel = job.cancel;
      await job.promise;
      // Служебный файл архива — не часть сборки: в проверке он был бы «лишним».
      fs.rmSync(path.join(stagingPath, gamePack.META_FILE), { force: true });

      this.packCancel = () => this.integrityService.cancel();
      this.setStatus(gameId, { state: "checking-files", percent: 0, checked: 0, totalChecked: filesManifest.files.length });
      const check = await this.integrityService.check({
        gameId,
        installPath: stagingPath,
        filesManifest,
        preservePaths: manifest.preservePaths || [],
      });
      this.packCancel = null;
      const modified = check.ok ? null : await this.restorePackOriginals(gameId, meta.version, stagingPath, filesManifest, check);

      this.setStatus(gameId, { state: "finalizing" });
      const config = manifestRegistry.launchConfigOf(gameId) || {};
      // Скачанные оригиналы подтверждены списком версии, как и прошедшие проверку файлы.
      const verified = modified ? Object.assign({}, check, { valid: check.valid.concat(modified.restoredPaths) }) : check;
      const { dropped } = gamePack.sanitize(stagingPath, verified, { dependencies: config.dependencies || [] });
      if (dropped.length)
        this.logger.warn(
          `[game-delivery] ${gameId}: из архива отброшено файлов — ${dropped.length}: ${dropped.slice(0, 10).join(", ")}`
        );

      this.assertGameNotRunning(gameId);
      const installPath = gameInstallPath(this.paths, gameId);
      const ctx = {
        gameId,
        manifest: { version: meta.version },
        installPath,
        stagingPath,
        publishedAt: meta.publishedAt,
        manifestSha256: meta.manifestSha256,
      };
      this.installService.swapIntoLibrary(ctx, stagingPath);
      const installed = this.installService.writeInstalled(ctx, meta.archiveSha256 || null);
      this.refreshBuildMetadataSnapshot(installed);

      this.updateChecker.invalidate(gameId);
      this.lastIntegrity.delete(gameId);
      this.logger.info(
        `[game-delivery] ${gameId}: сборка ${meta.version} импортирована из ${archivePath}, ` +
          `файлов сборки проверено ${check.checked}, отброшено ${dropped.length}` +
          (modified ? `, изменённых ${modified.changed}, оригиналов скачано ${modified.restored}` : "")
      );
      const where = (relative) => path.join(installPath, ...relative.split("/"));
      return {
        status: "ok",
        gameId,
        version: meta.version,
        dropped: dropped.length,
        modified: modified && {
          changed: modified.changed,
          restored: modified.restored,
          custom: modified.custom,
          customDir: modified.custom ? where(gamePack.customLayerOf(gameId)) : null,
          backup: modified.backup,
          backupDir: modified.backup ? where(gamePack.IMPORTED_BACKUP) : null,
          rejected: modified.rejected,
        },
      };
    } catch (error) {
      try {
        if (fs.existsSync(stagingPath)) removeWithinZone(this.paths.staging, stagingPath);
      } catch (cleanupError) {
        this.logger.warn(`[game-delivery] не удалось убрать ${stagingPath}: ${cleanupError.message}`);
      }
      if (error && error.code === "CANCELLED") this.setStatus(gameId, { state: "cancelled", percent: 0, error: null });
      throw error;
    } finally {
      this.packCancel = null;
      if (locked) this.installService.releaseLock();
      if (this.activeGameId === gameId) this.activeGameId = null;
      this.setOperation(gameId, null);
      this.releaseSlot(slot);
      this.refresh(gameId);
    }
  }

  // Эпизоды Ghosting (episodes.js, episode-install.js): папки episodic\ и ep2\ внутри
  // установленной ghosting-renewed. Операция идёт под её gameId: прогресс, пауза, отмена
  // и «занято» — как у установки игры.
  episodeIdOf(id) {
    if (typeof id !== "string" || !episodes.isEpisode(id))
      throw new DeliveryError("LAUNCH_CONFIG_INVALID", `Нет эпизода ${id}`, { id: String(id) });
    return id;
  }

  async fetchEpisodeManifest(id, { fresh = false } = {}) {
    const cached = this.episodeManifests.get(id);
    if (!fresh && cached && Date.now() - cached.fetchedAt < EPISODE_MANIFEST_TTL_MS) return cached;
    const failed = this.episodeManifestFailures.get(id);
    if (!fresh && failed && Date.now() - failed.at < EPISODE_MANIFEST_RETRY_MS) throw failed.error;
    try {
      const entry = Object.assign(await episodeInstall.fetchManifest(id, this.env), { fetchedAt: Date.now() });
      this.episodeManifests.set(id, entry);
      this.episodeManifestFailures.delete(id);
      return entry;
    } catch (error) {
      this.episodeManifestFailures.set(id, { error, at: Date.now() });
      this.logger.warn(`[game-delivery] манифест эпизода ${id} недоступен: ${error.code || ""} ${error.message}`);
      throw error;
    }
  }

  // Загрузки эпизодов на паузе — по файлам состояния .part.json в Downloads: кэш манифестов
  // после перезапуска лаунчера пуст.
  pausedEpisodes() {
    const gameId = episodeInstall.GAME_ID;
    const paused = {};
    let names = [];
    try {
      names = fs.readdirSync(path.join(this.paths.downloads, gameId));
    } catch (error) {
      return paused;
    }
    for (const name of names) {
      const fileName = name.endsWith(".part.json") ? name.slice(0, -".part.json".length) : null;
      const id = fileName ? episodeInstall.episodeOfArchive(fileName) : null;
      const resumable = id ? this.downloadManager.findResumable(gameId, fileName) : null;
      if (resumable) paused[id] = { fileName, bytesDownloaded: resumable.bytesDownloaded };
    }
    return paused;
  }

  // Архивы эпизода в Downloads (.part с состоянием или целый, оставленный отказом), кроме keepFileName:
  // прежняя версия (сервер выложил новую), отмена, удаление эпизода. Возвращает, сколько файлов убрано.
  discardStaleEpisodeArchives(id, keepFileName) {
    const dir = path.join(this.paths.downloads, episodeInstall.GAME_ID);
    let names = [];
    try {
      names = fs.readdirSync(dir);
    } catch (error) {
      return 0;
    }
    let removed = 0;
    for (const name of names) {
      const base = name.replace(/\.part(\.json)?$/, "");
      if (base === keepFileName || episodeInstall.episodeOfArchive(base) !== id) continue;
      try {
        removeWithinZone(this.paths.downloads, path.join(dir, name));
        removed++;
      } catch (error) {
        this.logger.warn(`[game-delivery] не удалось удалить архив эпизода ${name}: ${error.message}`);
      }
    }
    return removed;
  }

  // Архив, оставленный прошлой попыткой (игра была запущена, не хватило места): без второй загрузки.
  async keptEpisodeArchive(archive) {
    const file = path.join(this.paths.downloads, episodeInstall.GAME_ID, archive.fileName);
    try {
      if (fs.statSync(file).size !== archive.size) return null;
    } catch (error) {
      return null;
    }
    this.setStatus(episodeInstall.GAME_ID, { state: "verifying", percent: 0 });
    return (await fileOps.hashFile(file)) === archive.sha256 ? file : null;
  }

  episodesView(installed) {
    const state = episodes.status(installed ? installed.installPath : null);
    const paused = this.pausedEpisodes();
    const view = {};
    for (const id of Object.keys(state)) {
      const cached = this.episodeManifests.get(id);
      view[id] = Object.assign({}, state[id], {
        size: cached ? cached.manifest.archive.size : null,
        paused: !!(paused[id] && paused[id].bytesDownloaded > 0),
      });
    }
    return view;
  }

  // Для «Версии сборки»: не бросает, недоступный манифест — size null, available false.
  async episodesStatus() {
    const gameId = episodeInstall.GAME_ID;
    const available = {};
    await Promise.all(
      Object.keys(episodes.EPISODES).map(async (id) => {
        try {
          await this.fetchEpisodeManifest(id);
          available[id] = true;
        } catch (error) {
          available[id] = false;
        }
      })
    );
    const view = this.episodesView(this.installService.readInstalled(gameId));
    for (const id of Object.keys(view)) view[id].available = available[id];
    this.refresh(gameId);
    return view;
  }

  async installEpisode(id) {
    const gameId = episodeInstall.GAME_ID;
    this.episodeIdOf(id);
    const slot = this.reserveSlot(gameId, "episode-install");

    try {
      this.assertGameNotRunning(gameId);
      const installed = this.installedOf(gameId);
      ensureLayout(this.paths);
      this.activeGameId = gameId;
      this.setOperation(gameId, "episode-install");

      // Episode Two монтирует episodic\: без Episode One сначала ставится он, в том же слоте.
      let result = null;
      for (const each of episodes.EPISODES[id].requires.concat(id)) {
        if (each !== id && episodes.status(installed.installPath)[each].installed) continue;
        result = await this.runEpisodeInstall(each, installed.installPath);
        if (result.status !== "installed") return result;
      }
      return result;
    } catch (error) {
      const delivery = toDeliveryError(error, "INSTALL_FAILED");
      if (this.activeGameId === gameId)
        this.setStatus(gameId, { state: delivery.code === "CANCELLED" ? "cancelled" : "error", error: delivery.toPublic() });
      throw delivery;
    } finally {
      this.packCancel = null;
      if (this.activeGameId === gameId) this.activeGameId = null;
      this.setOperation(gameId, null);
      this.releaseSlot(slot);
      // Ошибка эпизода уходит в ответ IPC; в статусе Ghosting она читалась бы как беда самой игры (unavailable-reasons).
      this.statusOf(gameId).error = null;
      this.refresh(gameId);
    }
  }

  async runEpisodeInstall(id, installPath) {
    const gameId = episodeInstall.GAME_ID;
    const dir = episodes.EPISODES[id].dir;
    this.setStatus(gameId, { state: "preparing", episode: id, percent: 0, bytesDownloaded: 0, error: null });

    const { manifest, url } = await this.fetchEpisodeManifest(id, { fresh: true });
    const marker = episodes.readMarker(installPath, id);
    // Повтор старого подписанного манифеста не должен откатить эпизод; та же версия — переустановка.
    if (marker && typeof marker.version === "string" && semver.lt(manifest.version, marker.version))
      throw new DeliveryError("MANIFEST_INVALID", `Откат версии эпизода ${id}: ${marker.version} -> ${manifest.version}`, {
        id,
        installed: marker.version,
        offered: manifest.version,
      });

    const archive = manifest.archive;
    this.discardStaleEpisodeArchives(id, archive.fileName);
    this.setStatus(gameId, { totalBytes: archive.size });
    let archivePath = await this.keptEpisodeArchive(archive);
    if (!archivePath) {
      const allowedProviderTypes = featureFlag.allowedProviderTypes(this.env);
      const provider = providers.create(episodeInstall.providerDescriptor(manifest, url), { env: this.env, allowedProviderTypes });
      const download = await this.downloadManager.start({
        gameId,
        provider,
        filename: archive.fileName,
        expectedSize: archive.size,
        expectedSha256: archive.sha256,
      });
      if (download.status !== "completed") return download;
      archivePath = download.filePath;
    }

    const staging = assertInside(
      this.paths.staging,
      path.join(this.paths.staging, `${gameId}-episode-${id}-${crypto.randomBytes(4).toString("hex")}`),
      "Папка эпизода"
    );
    let locked = false;
    let dropArchive = true; // после установки или когда архив доказанно плохой
    try {
      this.installService.checkWritable();
      // Каждая запись — под <dir>/: ни .., ни абсолютных путей, ни второго корня — до распаковки.
      const { report } = await this.archiveService.inspect(archivePath, {
        maxEntries: gamePack.MAX_ENTRIES,
        maxUncompressedSize: gamePack.MAX_UNCOMPRESSED_BYTES,
        rootPrefix: dir,
      });
      this.installService.checkSpace(report.uncompressedSize || 1);
      this.installService.acquireLock(gameId);
      locked = true;

      fs.mkdirSync(staging, { recursive: true });
      this.setStatus(gameId, { state: "extracting", percent: 0 });
      const job = this.archiveService.extract(archivePath, staging, {
        onProgress: (fraction) => this.setStatus(gameId, { percent: Math.round(fraction * 100) }),
      });
      this.packCancel = job.cancel;
      await job.promise;
      this.packCancel = null;

      const fresh = path.join(staging, dir);
      if (!fs.existsSync(path.join(fresh, "gameinfo.txt")))
        throw new DeliveryError("EXTRACT_FAILED", `В архиве эпизода нет ${dir}/gameinfo.txt`, { id });

      this.setStatus(gameId, { state: "finalizing", percent: 100 });
      this.assertGameNotRunning(gameId);
      // config.cfg прошлого запуска эпизода, не вернувшийся из-за краха, — в Ghosting до подмены папки.
      episodes.syncConfigBack(installPath, id);
      // Хвост прошлой подмены, прерванной крахом: данные игрока из park — в эпизод, а не в .stale.
      this.installService.unparkEpisodes(gameId, installPath);
      episodeInstall.swapIn({
        installPath,
        id,
        fresh,
        park: this.installService.episodesParkPath(gameId),
        logger: this.logger,
        marker: {
          version: manifest.version,
          sha256: archive.sha256,
          installedAt: new Date().toISOString(),
          stockCfg: episodeInstall.stockCfgOf(fresh),
        },
      });
    } catch (error) {
      dropArchive = EPISODE_ARCHIVE_BAD.includes(error && error.code);
      throw error;
    } finally {
      this.packCancel = null;
      if (locked) this.installService.releaseLock();
      try {
        if (fs.existsSync(staging)) removeWithinZone(this.paths.staging, staging);
      } catch (cleanupError) {
        this.logger.warn(`[game-delivery] не удалось убрать ${staging}: ${cleanupError.message}`);
      }
      // Архив эпизода — гигабайты: убирается после установки или когда он доказанно плохой.
      if (dropArchive) {
        this.downloadManager.discard(gameId, archive.fileName);
        this.removeArchive(gameId, archive.fileName);
      }
    }

    this.logger.info(`[game-delivery] ${gameId}: эпизод ${id} ${manifest.version} установлен в ${dir}`);
    return { status: "installed", episode: id, version: manifest.version };
  }

  async removeEpisode(id) {
    const gameId = episodeInstall.GAME_ID;
    this.episodeIdOf(id);
    const slot = this.reserveSlot(gameId, "episode-remove");

    try {
      this.assertGameNotRunning(gameId);
      const installed = this.installedOf(gameId);
      // Бинды из эпизода, не вернувшиеся из-за краха, — в Ghosting, прежде чем папка исчезнет.
      for (const each of episodes.dependentsOf(id).concat(id)) {
        try {
          episodes.syncConfigBack(installed.installPath, each);
        } catch (error) {
          this.logger.warn(`[game-delivery] config.cfg эпизода ${each} не вернулся: ${error.message}`);
        }
        this.discardStaleEpisodeArchives(each, null);
      }
      const { removed } = episodes.remove(installed.installPath, id);
      this.logger.info(`[game-delivery] ${gameId}: удалены эпизоды ${removed.join(", ") || "—"}`);
      return { status: "removed", removed };
    } finally {
      this.releaseSlot(slot);
      this.refresh(gameId);
    }
  }

  processState(gameId) {
    return this.processService.getProcessState(gameId);
  }

  isGameRunning(gameId) {
    return this.processService.isRunning(gameId);
  }

  registerStartedProcess(gameId, processInfo) {
    const installed = this.installedOf(gameId);

    const registered = this.processService.registerStartedProcess(
      gameId,
      Object.assign({ installPath: installed.installPath }, processInfo || {})
    );

    this.refresh(gameId);
    return registered;
  }

  async stopGame(gameId, options) {
    const result = await this.processService.requestStop(gameId, options || {});
    this.refresh(gameId);
    return result;
  }

  reconcileProcesses(gameIds) {
    const results = this.processService.reconcileAfterLauncherRestart(gameIds);
    for (const item of results) this.refresh(item.gameId);
    // После сверки процессов известно, идёт ли игра: висящий config.cfg эпизода (крах игры или
    // лаунчера) возвращается, только если она не запущена.
    this.returnEpisodeConfigs(episodeInstall.GAME_ID);
    return results;
  }

  // config.cfg эпизодов (ids, по умолчанию все с отметкой) — в Ghosting, если она не запущена. Не бросает.
  returnEpisodeConfigs(gameId, ids = null) {
    if (gameId !== episodeInstall.GAME_ID || this.processService.isRunning(gameId)) return;
    let installed = null;
    try {
      installed = this.installService.readInstalled(gameId);
    } catch (error) {
      this.logger.warn(`[game-delivery] config.cfg эпизодов не вернулся: ${error.message}`);
    }
    if (installed) episodes.returnPending(installed.installPath, { logger: this.logger, ids });
  }

  dispose() {
    this.processService.dispose();
  }

  describeFlag() {
    return featureFlag.describe(this.env);
  }

  describeChannel(gameId) {
    return require("./channel-config").describe(gameId, this.env);
  }
}

module.exports = GameDeliveryService;
module.exports.STATES = STATES;
