"use strict";

const fs = require("fs");
const path = require("path");
const dns = require("dns");
const https = require("https");
const crypto = require("crypto");

const { resolveLibrary } = require("../game-delivery/paths");
const launcherChannel = require("../launcher-update/launcher-channel");
const buildMetadataTrust = require("../game-delivery/build-metadata-trust");
const buildMetadataReader = require("../game-delivery/build-metadata-reader");
const endpoints = require("../../shared/delivery-endpoints");
const ChannelService = require("../game-delivery/ChannelService");
const ProtonManager = require("../linux/ProtonManager");
const { readJsonState } = require("../game-delivery/atomic");
const { validSpeedLimit } = require("../../shared/download-settings");
const netFallback = require("../net-fallback");

const NET_TIMEOUT_MS = 6000;
const MAX_BODY = 512 * 1024;

const OWNERSHIP_TIMEOUT_MS = 15000;

const ENDPOINTS = {
  launcherChannel: endpoints.launcher.channelUrls[0],
  latestYml: endpoints.launcher.latestYmlUrls[0],
  // Тот же список, по которому лаунчер ищет новые сборки Proton Wineland.
  protonReleases: ProtonManager.RELEASES_API,
  gameChannel: endpoints.game.channelUrl("ghosting-renewed"),
  githubReleases: `${endpoints.GITHUB_RELEASE_BASE}/latest`,
  githubRateLimit: "https://api.github.com/rate_limit",
};

function isLinux(ctx) {
  return ((ctx && ctx.platform) || process.platform) === "linux";
}

function classifyNetworkError(error) {
  const code = error && error.code ? String(error.code) : "";
  const message = error && error.message ? error.message : "";

  if (code === "ENOTFOUND" || code === "EAI_AGAIN" || code === "ESERVFAIL") return "DNS_ERROR";
  if (code === "ETIMEDOUT" || /таймаут|timeout/i.test(message)) return "CONNECT_TIMEOUT";
  if (code === "ECONNRESET") return "CONNECTION_RESET";
  if (/^ERR_TLS|UNABLE_TO_VERIFY|CERT_|DEPTH_ZERO|SELF_SIGNED/i.test(code) || /TLS|certificate/i.test(message))
    return "TLS_ERROR";
  return "UNKNOWN_NETWORK_ERROR";
}

function classifyHttpStatus(status) {
  if (status === 404) return "HTTP_404_FEED_MISSING";
  if (status === 403) return "HTTP_403_FORBIDDEN";
  if (status === 429) return "HTTP_429_RATE_LIMIT";
  if (status < 200 || status >= 300) return "UNKNOWN_NETWORK_ERROR";
  return null;
}

function httpsGet(url, cancel, redirectsLeft) {
  const left = redirectsLeft == null ? 5 : redirectsLeft;

  return new Promise((resolve, reject) => {
    if (cancel.cancelled) return reject(Object.assign(new Error("отменено"), { code: "CANCELLED" }));

    const request = https.get(
      url,
      { headers: { "user-agent": "HLSR-diagnostics" }, timeout: NET_TIMEOUT_MS },
      (response) => {
        const status = response.statusCode;

        if (status >= 301 && status <= 308 && response.headers.location && left > 0) {
          response.resume();
          return resolve(httpsGet(new URL(response.headers.location, url).toString(), cancel, left - 1));
        }

        const chunks = [];
        let total = 0;

        response.on("data", (chunk) => {
          total += chunk.length;
          if (total > MAX_BODY) {
            response.destroy();
            return resolve({ status, headers: response.headers, body: null, truncated: true });
          }
          chunks.push(chunk);
        });
        response.on("end", () =>
          resolve({ status, headers: response.headers, body: Buffer.concat(chunks) })
        );
        response.on("error", reject);
      }
    );

    cancel.abort.push(() => request.destroy(Object.assign(new Error("отменено"), { code: "CANCELLED" })));
    request.on("timeout", () => request.destroy(Object.assign(new Error("таймаут"), { code: "ETIMEDOUT" })));
    request.on("error", reject);
  });
}

// TLS-ошибку Node перепроверяем через Chromium: если там адрес открылся,
// виноват список сертификатов Node, а не сеть, и проверка проходит.
async function requestWithFallback(url, cancel) {
  try {
    return await httpsGet(url, cancel);
  } catch (error) {
    if (cancel.cancelled || !netFallback.isTlsError(error) || !netFallback.electronNet()) throw error;
    const controller = new AbortController();
    const abort = () => controller.abort();
    cancel.abort.push(abort);
    try {
      return await netFallback.chromiumGet(url, {
        headers: { "user-agent": "HLSR-diagnostics" },
        signal: controller.signal,
        limit: MAX_BODY,
      });
    } catch (fallbackError) {
      throw error;
    } finally {
      const index = cancel.abort.indexOf(abort);
      if (index >= 0) cancel.abort.splice(index, 1);
    }
  }
}

function errorDetail(error) {
  const code = error && error.code ? String(error.code) : "";
  const message = error && error.message ? String(error.message).slice(0, 120) : "";
  return [code, message].filter(Boolean).join(": ");
}

function safeCall(fn) {
  try {
    return fn();
  } catch (error) {
    return null;
  }
}

function writeProbe(dir) {
  const probe = path.join(dir, `.hlsr-diag-${process.pid}-${crypto.randomBytes(6).toString("hex")}`);
  let created = false;
  try {
    fs.writeFileSync(probe, "", { flag: "wx", mode: 0o600 });
    created = true;
    fs.unlinkSync(probe);
    created = false;
    return true;
  } catch (error) {
    return false;
  } finally {
    if (created) {
      try { fs.unlinkSync(probe); } catch (ignored) {}
    }
  }
}

function check(id, group, status, options) {
  return Object.assign({ id, group, status, code: null, detail: "" }, options || {});
}

class DiagnosticsRunner {
  constructor(options) {
    const opts = options || {};
    this.logger = opts.logger || { info() {}, warn() {} };
    this.context = opts.context || {};
    this.request = opts.request || requestWithFallback;
    this.lookup = opts.lookup || dns.lookup;
    this.channelService = opts.channelService || new ChannelService();
    this.launcherPublicKeys = opts.launcherPublicKeys || launcherChannel.LAUNCHER_PUBLIC_KEYS;
    this.running = false;
    this.cancelToken = null;
  }

  cancel() {
    if (this.cancelToken) {
      this.cancelToken.cancelled = true;
      for (const abort of this.cancelToken.abort.splice(0)) {
        try {
          abort();
        } catch (e) {
          /* уже завершён */
        }
      }
    }
  }

  awaitCancelable(value) {
    const token = this.cancelToken;
    return new Promise((resolve, reject) => {
      let settled = false;
      const finish = (error, result) => {
        if (settled) return;
        settled = true;
        const index = token.abort.indexOf(abort);
        if (index >= 0) token.abort.splice(index, 1);
        error ? reject(error) : resolve(result);
      };
      const abort = () => finish(Object.assign(new Error("отменено"), { code: "CANCELLED" }));
      token.abort.push(abort);
      Promise.resolve(value).then((result) => finish(null, result), (error) => finish(error));
      if (token.cancelled) abort();
    });
  }

  async run(emit) {
    if (this.running) return { status: "already-running", checks: [] };

    this.running = true;
    this.cancelToken = { cancelled: false, abort: [] };
    const checks = [];
    const push = (item) => {
      checks.push(item);
      if (emit)
        try {
          emit(item);
        } catch (e) {
          /* подписчик не роняет диагностику */
        }
    };

    try {
      const ctx = typeof this.context === "function" ? this.context() : this.context;

      this.runHlsrChecks(ctx, push);
      await this.runGpuChecks(ctx, push);
      this.runDownloadChecks(ctx, push);
      if (isLinux(ctx)) this.runLinuxChecks(ctx, push);
      else this.runWindowsChecks(push);
      this.runLibraryChecks(ctx, push);
      this.runBuildMetadataChecks(ctx, push);
      this.runProfileChecks(ctx, push);
      await this.runSteamChecks(ctx, push);
      await this.runNetworkChecks(push, ctx);

      return { status: this.cancelToken.cancelled ? "cancelled" : "ok", checks };
    } catch (error) {
      if (this.cancelToken.cancelled) return { status: "cancelled", checks };
      throw error;
    } finally {
      this.cancelToken.abort.length = 0;
      this.running = false;
    }
  }

  runHlsrChecks(ctx, push) {
    push(check("hlsr-version", "hlsr", "ok", { detail: `${ctx.appVersion || "?"} (${ctx.packaged ? "packaged" : "dev"})` }));
    push(check("hlsr-arch", "hlsr", process.arch === "x64" ? "ok" : "warn", { detail: process.arch }));
    if (isLinux(ctx))
      push(
        check("hlsr-umu", "hlsr", ctx.umuAvailable ? "ok" : "warn", {
          code: ctx.umuAvailable ? null : "UMU_NOT_FOUND",
        })
      );
    else
      push(
        check("hlsr-native", "hlsr", ctx.nativeAvailable ? "ok" : "warn", {
          code: ctx.nativeAvailable ? null : "NATIVE_UNAVAILABLE",
          detail: ctx.nativeAvailable ? "" : String(ctx.nativeLoadError || "").slice(0, 500),
        })
      );

    const writable = !!ctx.userDataPath && writeProbe(ctx.userDataPath);
    push(check("hlsr-userdata", "hlsr", writable ? "ok" : "fail", { code: writable ? null : "USERDATA_READONLY" }));

    const updateFailed = ctx.updateState === "error";
    push(check("hlsr-updater", "hlsr", updateFailed ? "fail" : ctx.updateEnabled === false ? "warn" : "ok", {
      detail: ctx.updateState || "idle",
      code: updateFailed ? "UPDATER_ERROR" : ctx.updateEnabled === false ? "UPDATER_DISABLED" : null,
    }));
  }

  async runGpuChecks(ctx, push) {
    let gpu = null;
    try {
      if (typeof ctx.gpuStatus === "function") gpu = await this.awaitCancelable(ctx.gpuStatus());
    } catch (error) {
      if (error.code === "CANCELLED") throw error;
    }
    if (!gpu) {
      push(check("gpu-runtime", "graphics", "warn", { code: "GPU_STATUS_UNKNOWN" }));
      return;
    }
    push(check("gpu-configured", "graphics", "ok", {
      detailKey: gpu.configuredEnabled ? "#L_DGV_ENABLED" : "#L_DGV_DISABLED",
    }));
    const pending = typeof gpu.appliedEnabled === "boolean" && gpu.configuredEnabled !== gpu.appliedEnabled;
    push(check("gpu-applied", "graphics", gpu.appliedEnabled == null || pending ? "warn" : "ok", {
      code: pending ? "GPU_RESTART_REQUIRED" : gpu.appliedEnabled == null ? "GPU_STATUS_UNKNOWN" : null,
      detailKey: gpu.appliedEnabled == null ? "#L_DGV_UNKNOWN" : gpu.appliedEnabled ? "#L_DGV_ENABLED" : "#L_DGV_DISABLED",
    }));
    if (!gpu.available) {
      push(check("gpu-runtime", "graphics", "warn", { code: "GPU_STATUS_UNKNOWN" }));
      return;
    }
    const hardware = gpu.hardwareEnabled && String(gpu.compositing || "").startsWith("enabled") && !gpu.softwareRenderer;
    const disableFailed = gpu.appliedEnabled === false && gpu.hardwareEnabled === true;
    const fallback = gpu.appliedEnabled === true && !hardware;
    push(check("gpu-runtime", "graphics", disableFailed ? "fail" : fallback ? "warn" : "ok", {
      code: disableFailed ? "GPU_DISABLE_FAILED" : fallback ? "GPU_SOFTWARE_FALLBACK" : null,
      detailKey: hardware ? "#L_DGV_GPU_HARDWARE" : "#L_DGV_GPU_SOFTWARE",
    }));
    if (gpu.device) push(check("gpu-device", "graphics", "ok", { detail: gpu.device }));
    if (gpu.driver) push(check("gpu-driver", "graphics", "ok", { detail: gpu.driver }));
  }

  runDownloadChecks(ctx, push) {
    if (!ctx.userDataPath) {
      push(check("downloads-limit", "downloads", "warn", { code: "DOWNLOAD_SETTINGS_INVALID" }));
      return;
    }
    const saved = readJsonState(path.join(ctx.userDataPath, "download-settings.json"));
    const limit = saved.valid && saved.data ? saved.data.speedLimit : null;
    if (saved.found && (!saved.valid || !validSpeedLimit(limit))) {
      push(check("downloads-limit", "downloads", "fail", { code: "DOWNLOAD_SETTINGS_INVALID" }));
      return;
    }
    const speedLimit = saved.found ? limit : 0;
    push(check("downloads-limit", "downloads", "ok", {
      detailKey: speedLimit ? "#L_S_MBPS" : "#L_S_NO_LIMIT",
      detailArgs: speedLimit ? [String(speedLimit)] : [],
    }));
    const effective = typeof ctx.downloadLimitBps === "function" ? safeCall(ctx.downloadLimitBps) : ctx.downloadLimitBps;
    if (Number.isFinite(effective)) {
      const applied = effective === speedLimit * 1024 * 1024;
      push(check("downloads-applied", "downloads", applied ? "ok" : "warn", {
        code: applied ? null : "DOWNLOAD_LIMIT_NOT_APPLIED",
      }));
    }
  }

  runWindowsChecks(push) {
    const os = require("os");
    push(check("win-version", "windows", "ok", { detail: `Windows ${os.release()}` }));
    push(check("win-arch", "windows", os.arch() === "x64" ? "ok" : "warn", { detail: os.arch() }));

    const year = new Date().getFullYear();
    const sane = year >= 2020 && year <= 2100;
    push(
      check("win-clock", "windows", sane ? "ok" : "warn", {
        code: sane ? null : "CLOCK_SUSPECT",
        detail: new Date().toISOString().slice(0, 10),
      })
    );
  }

  runLinuxChecks(ctx, push) {
    const os = require("os");
    push(check("linux-kernel", "linux", "ok", { detail: os.release() }));
    push(check("linux-arch", "linux", os.arch() === "x64" ? "ok" : "warn", { detail: os.arch() }));
    push(
      check("linux-proton", "linux", ctx.protonSelected ? "ok" : "warn", {
        code: ctx.protonSelected ? null : "PROTON_NOT_INSTALLED",
        detail: ctx.protonSelected || "",
      })
    );

    const year = new Date().getFullYear();
    const sane = year >= 2020 && year <= 2100;
    push(
      check("linux-clock", "linux", sane ? "ok" : "warn", {
        code: sane ? null : "CLOCK_SUSPECT",
        detail: new Date().toISOString().slice(0, 10),
      })
    );
  }

  runLibraryChecks(ctx, push) {
    const root = ctx.libraryPath;

    let paths = null;
    try {
      paths = resolveLibrary(root);
      push(check("lib-root", "library", "ok", { detail: root }));
    } catch (e) {
      push(check("lib-root", "library", "fail", { code: "LIBRARY_UNAVAILABLE", detail: String(root) }));
      return;
    }

    const exists = fs.existsSync(paths.root);
    push(check("lib-exists", "library", exists ? "ok" : "warn", { code: exists ? null : "LIBRARY_MISSING" }));

    if (exists) {
      const writable = writeProbe(paths.root);
      push(check("lib-writable", "library", writable ? "ok" : "fail", { code: writable ? null : "LIBRARY_READ_ONLY" }));

      for (const key of ["games", "downloads", "internal"])
        push(
          check(`lib-${key}`, "library", fs.existsSync(paths[key]) ? "ok" : "warn", {
            detail: path.basename(paths[key]),
          })
        );
    }

    const freeSpaceMb = Number.isFinite(ctx.freeSpaceMb) ? ctx.freeSpaceMb : null;

    if (freeSpaceMb === null)
      push(check("lib-space", "library", "warn", { code: "DISK_SPACE_UNKNOWN" }));
    else
      push(
        check("lib-space", "library", freeSpaceMb > 6000 ? "ok" : "warn", {
          code: freeSpaceMb > 6000 ? null : "LOW_DISK_SPACE",
          detail: `${(freeSpaceMb / 1024).toFixed(1)} GB`,
        })
      );

    const installedFile = path.join(paths.games, "ghosting-renewed", "installed.json");
    if (fs.existsSync(installedFile)) {
      let ok = false;
      let version = null;
      try {
        const data = JSON.parse(fs.readFileSync(installedFile, "utf8"));
        ok = data && data.gameId === "ghosting-renewed" && data.state === "ready";
        version = data ? data.version : null;
      } catch (e) {
        ok = false;
      }
      push(
        check("lib-installed", "library", ok ? "ok" : "fail", {
          code: ok ? null : "INSTALLED_JSON_INVALID",
          detail: version || "",
        })
      );
    } else {
      push(check("lib-installed", "library", "warn", { code: "GAME_NOT_INSTALLED" }));
    }
  }

  runBuildMetadataChecks(ctx, push) {
    let paths;
    try {
      paths = resolveLibrary(ctx.libraryPath);
    } catch (e) {
      return;
    }

    const installedFile = path.join(paths.games, "ghosting-renewed", "installed.json");
    if (!fs.existsSync(installedFile)) {
      push(check("build-meta-present", "build", "warn", { code: "GAME_NOT_INSTALLED" }));
      return;
    }

    let installed = null;
    try {
      installed = JSON.parse(fs.readFileSync(installedFile, "utf8"));
    } catch (e) {
      installed = null;
    }
    if (!installed) {
      push(check("build-meta-present", "build", "fail", { code: "INSTALLED_JSON_INVALID" }));
      return;
    }

    const snapshotMeta = installed.buildMetadata || null;

    if (!snapshotMeta) {
      push(check("build-meta-present", "build", "warn", { code: "BUILD_METADATA_LEGACY", detail: "legacy" }));
      push(
        check("build-meta-date", "build", installed.publishedAt ? "ok" : "warn", {
          detail: installed.publishedAt ? `publishedAt ${String(installed.publishedAt).slice(0, 10)}` : "Нет данных",
        })
      );
      push(check("build-meta-trust", "build", "ok", { code: "BUILD_METADATA_LEGACY", detail: "legacy" }));
      return;
    }

    let runtime;
    try {
      runtime = buildMetadataTrust.verifyAgainstSnapshot({
        installRoot: installed.installPath,
        snapshot: installed,
        launcherVersion: ctx.appVersion,
      });
    } catch (e) {
      runtime = { state: "repair-required", reason: "BUILD_METADATA_INVALID" };
    }

    const reason = runtime.reason || null;
    const trusted = runtime.state === "trusted";

    push(
      check("build-meta-present", "build", reason === "BUILD_METADATA_MISSING" ? "fail" : "ok", {
        code: reason === "BUILD_METADATA_MISSING" ? reason : null,
        detail: snapshotMeta.buildName || "",
      })
    );

    push(
      check("build-meta-schema", "build", reason === "BUILD_METADATA_UNSUPPORTED" ? "fail" : "ok", {
        code: reason === "BUILD_METADATA_UNSUPPORTED" ? reason : null,
        detail: `schema ${snapshotMeta.schemaVersion || 1}`,
      })
    );

    push(
      check("build-meta-identity", "build", trusted ? "ok" : reason === "BUILD_METADATA_MISMATCH" ? "fail" : "warn", {
        code: reason === "BUILD_METADATA_MISMATCH" ? reason : null,
        detail: snapshotMeta.buildId || "",
      })
    );

    push(
      check("build-meta-version", "build", "ok", {
        detail: `${snapshotMeta.version || "?"} (${snapshotMeta.architecture || "?"})`,
      })
    );

    let hashOk = null;
    if (installed.buildMetadataSha256) {
      try {
        const readResult = buildMetadataReader.read(installed.installPath, { now: ctx.now });
        hashOk = readResult.present && readResult.sha256 === installed.buildMetadataSha256;
      } catch (e) {
        hashOk = false;
      }
      push(
        check("build-meta-hash", "build", hashOk ? "ok" : "fail", {
          code: hashOk ? null : "BUILD_METADATA_MISMATCH",
          detail: hashOk ? `sha ${installed.buildMetadataSha256.slice(0, 12)}…` : "не совпал",
        })
      );
    }

    const dateSource = snapshotMeta.builtAt ? "builtAt" : installed.publishedAt ? "publishedAt" : null;
    push(
      check("build-meta-date", "build", dateSource ? "ok" : "warn", {
        detail: dateSource === "builtAt"
          ? `builtAt ${String(snapshotMeta.builtAt).slice(0, 10)}`
          : dateSource === "publishedAt"
            ? `publishedAt ${String(installed.publishedAt).slice(0, 10)}`
            : "Нет данных",
      })
    );

    push(
      check("build-meta-launcher", "build", reason === "BUILD_REQUIRES_NEWER_LAUNCHER" ? "fail" : "ok", {
        code: reason === "BUILD_REQUIRES_NEWER_LAUNCHER" ? reason : null,
        detail: `>= ${snapshotMeta.minimumLauncherVersion || "?"}`,
      })
    );

    push(
      check("build-meta-trust", "build", trusted ? "ok" : runtime.state === "launcher-update-required" ? "warn" : "fail", {
        code: reason,
        detail: runtime.state,
      })
    );
  }

  async runSteamChecks(ctx, push) {
    const health = ctx.steamHealth || { status: "unknown", code: "STEAM_CHECK_FAILED" };

    push(
      check("steam-running", "steam", health.status === "available" ? "ok" : health.status === "unknown" ? "warn" : "fail", {
        code: health.code || null,
        detail: health.detail || "",
      })
    );
    let launch = null;
    try {
      if (typeof ctx.launchAvailability === "function")
        launch = await this.awaitCancelable(ctx.launchAvailability());
    } catch (error) {
      if (error.code === "CANCELLED") throw error;
    }
    const launchDetails = {
      STEAM_NOT_INSTALLED: "#L_S_LAUNCH_STEAM_NOT_INSTALLED",
      STEAM_NOT_RUNNING: "#L_S_LAUNCH_STEAM_NOT_RUNNING",
      STEAM_PROCESS_MISMATCH: "#L_S_LAUNCH_STEAM_NOT_RUNNING",
      SOURCE_SDK_NOT_INSTALLED: "#L_S_LAUNCH_SDK_MISSING",
      SOURCE_SDK_INCOMPLETE: "#L_S_LAUNCH_SDK_MISSING",
      PROTON_NOT_INSTALLED: "#L_S_PROTON_EMPTY",
      UMU_NOT_FOUND: "#L_DGV_UMU_MISSING",
    };
    push(check("steam-launch", "steam", launch && launch.ok ? "ok" : "warn", {
      detailKey: launch && launch.ok ? "#L_DGV_LAUNCH_READY" : launch && launchDetails[launch.code],
      code: launch && !launch.ok ? launch.code || "LAUNCH_CONFIG_INVALID" : launch ? null : "STEAM_CHECK_FAILED",
    }));

    await this.runOwnershipChecks(ctx, push);
  }

  async runOwnershipChecks(ctx, push) {
    const apps = typeof ctx.requiredSteamApps === "function" ? ctx.requiredSteamApps() : [];
    if (!apps || !apps.length || typeof ctx.steamOwnership !== "function") return;

    await Promise.resolve();
    if (this.cancelToken && this.cancelToken.cancelled) return;

    for (const app of apps) {
      if (this.cancelToken && this.cancelToken.cancelled) return;

      let verdict;
      try {
        verdict = await this.awaitCancelable(ctx.steamOwnership(app.appId, { timeoutMs: OWNERSHIP_TIMEOUT_MS }));
      } catch (error) {
        if (error.code === "CANCELLED") throw error;
        verdict = { owned: null, code: "STEAM_CHECK_FAILED" };
      }

      const owned = verdict ? verdict.owned : null;
      const suffix = `${app.title} (AppID ${app.appId})`;

      push(
        check(`steam-owns-${app.appId}`, "steam", owned === true ? "ok" : owned === false ? "fail" : "warn", {
          code: owned === true ? null : owned === false ? "STEAM_APP_NOT_OWNED" : "LICENSE_CHECK_UNAVAILABLE",
          detail: suffix,
        })
      );
    }
  }

  /**
   * Профиль, статистика и связь с сервером.
   *
   * Отвечает на вопросы, которые чаще всего приходят в поддержку и на
   * которые раньше нельзя было ответить, не сидя за чужим компьютером:
   * «почему у меня нет значка», «почему часы не растут», «почему друзья
   * меня не видят». Каждый из них — своя строчка, а не общее «профиль
   * работает».
   *
   * Числа берутся из ОДНОГО источника с окном профиля: второй расчёт
   * тех же часов однажды разошёлся бы с первым, и отчёт описывал бы не
   * тот лаунчер, на который жалуются.
   */
  runProfileChecks(ctx, push) {
    const snapshot = typeof ctx.profile === "function" ? safeCall(ctx.profile) : null;

    if (!snapshot) {
      push(check("profile-state", "profile", "warn", { code: "PROFILE_STATE_UNKNOWN" }));
      return;
    }

    push(
      check("profile-steam", "profile", snapshot.signedIn ? "ok" : "warn", {
        code: snapshot.signedIn ? null : "STEAM_SIGNED_OUT",
      })
    );

    const seconds = Math.max(0, Math.floor(Number(snapshot.playSeconds) || 0));
    const hours = Math.floor(seconds / 3600);
    push(
      check("profile-playtime", "profile", "ok", {
        detail: `${hours}h ${Math.floor(seconds / 60) % 60}m ${seconds % 60}s`,
        detailKey: "#L_DGV_PLAYTIME",
        detailArgs: [hours, Math.floor(seconds / 60) % 60, seconds % 60, snapshot.launchCount, snapshot.gamesPlayed],
      })
    );

    if (snapshot.launchCount > 0 && snapshot.playSeconds <= 0)
      push(check("profile-playtime-stuck", "profile", "warn", { code: "PLAYTIME_NOT_COUNTED" }));

    push(
      check("profile-top-game", "profile", snapshot.topGameId ? "ok" : "warn", {
        code: snapshot.topGameId ? null : "NO_TOP_GAME",
        detail: snapshot.topGameId || "",
      })
    );

    if (snapshot.achievements)
      push(
        check("profile-achievements", "profile", "ok", {
          detail: `${snapshot.achievements.unlocked}/${snapshot.achievements.total}`,
        })
      );

    push(
      check("profile-since", "profile", snapshot.installedAt ? "ok" : "warn", {
        code: snapshot.installedAt ? null : "INSTALL_DATE_UNKNOWN",
        detail: snapshot.installedAt ? String(snapshot.installedAt).slice(0, 10) : "",
      })
    );

    if (typeof ctx.telemetryStatus === "function") {
      const telemetry = safeCall(ctx.telemetryStatus);
      push(check("profile-telemetry", "profile", telemetry ? telemetry.backendConfigured ? "ok" : "skip" : "warn", {
        code: telemetry ? telemetry.backendConfigured ? null : "TELEMETRY_NOT_CONFIGURED" : "PROFILE_STATE_UNKNOWN",
        detailKey: !telemetry ? "#L_DGV_UNKNOWN" : !telemetry.backendConfigured ? "#L_DGV_TELEMETRY_OFFLINE" : telemetry.enabled ? "#L_DGV_ENABLED" : "#L_DGV_DISABLED",
      }));
    } else if (typeof ctx.telemetryEnabled === "function")
      push(
        check("profile-telemetry", "profile", "ok", {
          detail: safeCall(ctx.telemetryEnabled) ? "включена" : "выключена",
        })
      );
  }

  async runNetworkChecks(push, ctx) {
    const cancel = this.cancelToken;
    if (cancel.cancelled) return;
    try {
      await new Promise((resolve, reject) => {
        let settled = false;
        const finish = (error) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          const index = cancel.abort.indexOf(abort);
          if (index >= 0) cancel.abort.splice(index, 1);
          error ? reject(error) : resolve();
        };
        const abort = () => finish(Object.assign(new Error("отменено"), { code: "CANCELLED" }));
        const timer = setTimeout(() => finish(Object.assign(new Error("таймаут"), { code: "ETIMEDOUT" })), NET_TIMEOUT_MS);
        cancel.abort.push(abort);
        try { this.lookup("github.com", finish); }
        catch (error) { finish(error); }
      });
      push(check("net-dns", "network", "ok", { detail: "github.com" }));
    } catch (error) {
      if (error.code === "CANCELLED") return;
      push(check("net-dns", "network", "fail", { code: classifyNetworkError(error) }));
    }
    if (cancel.cancelled) return;

    let sawForbidden = false;
    const probe = async (id, url, validate) => {
      if (cancel.cancelled) return;
      try {
        const response = await this.awaitCancelable(this.request(url, cancel));
        const httpCode = classifyHttpStatus(response.status) || (validate && response.status !== 200 ? "UNKNOWN_NETWORK_ERROR" : null);
        if (httpCode) {
          if (response.status === 403) sawForbidden = true;
          push(check(id, "network", "fail", { code: httpCode, detail: `HTTP ${response.status}` }));
          return;
        }
        if (validate) {
          const verdict = validate(response);
          if (verdict) {
            push(check(id, "network", "fail", { code: verdict }));
            return;
          }
        }
        push(check(id, "network", "ok", { detail: `HTTP ${response.status}` }));
      } catch (error) {
        if (error.code === "CANCELLED") return;
        push(check(id, "network", "fail", { code: classifyNetworkError(error), detail: errorDetail(error) }));
      }
    };
    const platform = isLinux(ctx) ? "linux" : "win32";
    const feed = endpoints.launcherFor(platform);
    const validateLauncher = (response) => {
      if (!response.body || response.truncated) return "CHECKSUM_METADATA_INVALID";
      let manifest;
      try { manifest = JSON.parse(response.body.toString("utf8")); }
      catch (error) { return "CHECKSUM_METADATA_INVALID"; }
      const verdict = launcherChannel.check(manifest, { platform, publicKeys: this.launcherPublicKeys });
      return verdict.ok ? null : verdict.code === launcherChannel.CODES.SIGNATURE_INVALID ? "SIGNATURE_INVALID" : "CHECKSUM_METADATA_INVALID";
    };
    const tasks = [
      probe("net-launcher-channel", feed.channelUrls[0], validateLauncher),
      probe("net-launcher-fallback", feed.channelUrls[1], validateLauncher),
    ];
    if (isLinux(ctx)) tasks.push(probe("net-proton-releases", ENDPOINTS.protonReleases, (response) => {
      if (!response.body || response.truncated) return "CHECKSUM_METADATA_INVALID";
      try {
        const list = JSON.parse(response.body.toString("utf8"));
        return Array.isArray(list) && list.some((release) => ProtonManager.updateCandidate(release) ||
          (release && release.tag_name === ProtonManager.RELEASE_TAG))
          ? null : "CHECKSUM_METADATA_INVALID";
      } catch (error) { return "CHECKSUM_METADATA_INVALID"; }
    }));
    else tasks.push(probe("net-latest-yml", feed.latestYmlUrls[0], (response) => {
      if (!response.body || response.truncated) return "CHECKSUM_METADATA_INVALID";
      try {
        const data = require("js-yaml").load(response.body.toString("utf8"));
        return data && typeof data.version === "string" && Array.isArray(data.files) && data.files.length ? null : "CHECKSUM_METADATA_INVALID";
      } catch (error) { return "CHECKSUM_METADATA_INVALID"; }
    }));
    tasks.push(probe("net-game-channel", ENDPOINTS.gameChannel, (response) => {
      if (!response.body || response.truncated) return "CHECKSUM_METADATA_INVALID";
      try {
        const data = JSON.parse(response.body.toString("utf8"));
        const config = this.channelService.configFor("ghosting-renewed");
        this.channelService.validate(data, "ghosting-renewed", config);
        return null;
      } catch (error) {
        return error.code === "SIGNATURE_INVALID" ? "SIGNATURE_INVALID" : "CHECKSUM_METADATA_INVALID";
      }
    }));
    tasks.push(probe("net-github", ENDPOINTS.githubReleases));
    await Promise.all(tasks);

    if (sawForbidden && !cancel.cancelled) {
      try {
        const response = await this.awaitCancelable(this.request(ENDPOINTS.githubRateLimit, cancel));
        const httpCode = classifyHttpStatus(response.status);
        if (httpCode) {
          push(check("net-rate-limit", "network", "warn", { code: httpCode }));
          return;
        }
        let data;
        try { data = JSON.parse(response.body.toString("utf8")); }
        catch (error) { data = null; }
        const remaining = data && data.resources && data.resources.core && data.resources.core.remaining;
        if (!Number.isFinite(remaining)) {
          push(check("net-rate-limit", "network", "warn", { code: "CHECKSUM_METADATA_INVALID" }));
          return;
        }
        const limited = remaining === 0;
        push(check("net-rate-limit", "network", limited ? "fail" : "ok", { code: limited ? "GITHUB_API_RATE_LIMIT" : null }));
      } catch (error) {
        if (error.code !== "CANCELLED") push(check("net-rate-limit", "network", "warn", { code: classifyNetworkError(error) }));
      }
    }
  }

}

module.exports = DiagnosticsRunner;
module.exports.ENDPOINTS = ENDPOINTS;
module.exports.classifyNetworkError = classifyNetworkError;
module.exports.classifyHttpStatus = classifyHttpStatus;
module.exports.NET_TIMEOUT_MS = NET_TIMEOUT_MS;
