"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { EventEmitter } = require("node:events");
const { performance } = require("node:perf_hooks");
const vm = require("node:vm");
const { createRequire } = require("node:module");
const gpu = require("../src/main/gpu-preference");
const Runner = require("../src/main/diagnostics/DiagnosticsRunner");
const ChannelService = require("../src/main/game-delivery/ChannelService");
const signing = require("../src/main/game-delivery/manifest-signing");
const endpoints = require("../src/shared/delivery-endpoints");
const ProtonManager = require("../src/main/linux/ProtonManager");

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-diagnostics-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const context = {
    userDataPath: root, libraryPath: root, appVersion: "0.3.16", platform: "linux",
    steamHealth: { status: "available" }, launchAvailability: () => ({ ok: true }),
  };
  return { root, context };
}

function settings(root, enabled, bom = false) {
  fs.writeFileSync(path.join(root, "settings.json"), (bom ? "\uFEFF" : "") + JSON.stringify({ config: { gpuAcceleration: enabled } }));
}

function fakeApp(root) {
  const app = new EventEmitter();
  app.hardware = true;
  app.calls = 0;
  app.switches = [];
  app.getPath = () => root;
  app.disableHardwareAcceleration = () => { app.hardware = false; };
  app.commandLine = { appendSwitch: (name) => app.switches.push(name) };
  app.isHardwareAccelerationEnabled = () => { app.calls++; return app.hardware; };
  app.getGPUFeatureStatus = () => ({ gpu_compositing: app.hardware ? "enabled" : "disabled_software" });
  app.getGPUInfo = async () => ({ gpuDevice: [{ active: true, vendorId: 4098, deviceString: "Test AMD", driverVersion: "1.0" }] });
  return app;
}

async function offline(context) {
  const runner = new Runner({ context });
  runner.runNetworkChecks = async () => {};
  return runner.run();
}

const row = (result, id) => result.checks.find((item) => item.id === id);

test("saved GPU setting controls startup; a later save requires another startup", async (t) => {
  const { root } = fixture(t);
  settings(root, false, true);
  const disabled = fakeApp(root);
  assert.equal(gpu.apply(disabled), false);
  assert.equal(disabled.hardware, false);
  assert.ok(disabled.switches.includes("disable-gpu-compositing"));
  disabled.emit("gpu-info-update");
  settings(root, true);
  const state = await gpu.snapshot(disabled);
  assert.equal(state.configuredEnabled, true);
  assert.equal(state.appliedEnabled, false);
  assert.equal(state.hardwareEnabled, false);
  const restarted = fakeApp(root);
  assert.equal(gpu.apply(restarted), true);
  restarted.emit("gpu-info-update");
  assert.equal((await gpu.snapshot(restarted)).hardwareEnabled, true);
});

test("GPU diagnostics wait for initialized information and do not leak listeners", async (t) => {
  const { root } = fixture(t);
  const app = fakeApp(root);
  gpu.apply(app);
  for (let i = 0; i < 3; i++) assert.equal((await gpu.snapshot(app, { timeoutMs: 5 })).available, false);
  assert.equal(app.calls, 0);
  assert.equal(app.listenerCount("gpu-info-update"), 1);
  app.emit("gpu-info-update");
  assert.equal((await gpu.snapshot(app)).available, true);
});

test("GPU driver failure preserves known rendering flags without inventing a device", async (t) => {
  const { root } = fixture(t);
  const app = fakeApp(root);
  gpu.apply(app);
  app.emit("gpu-info-update");
  app.getGPUInfo = async () => { throw new Error("driver information unavailable"); };
  const state = await gpu.snapshot(app);
  assert.equal(state.available, true);
  assert.equal(state.hardwareEnabled, true);
  assert.equal(state.device, "");
});

test("GPU status distinguishes pending settings, driver fallback and failed disable", async (t) => {
  const { context } = fixture(t);
  const base = { configuredEnabled: true, appliedEnabled: true, available: true, hardwareEnabled: true, compositing: "enabled", softwareRenderer: false };
  let result = await offline({ ...context, gpuStatus: () => base });
  assert.equal(row(result, "gpu-runtime").status, "ok");
  result = await offline({ ...context, gpuStatus: () => ({ ...base, configuredEnabled: false }) });
  assert.equal(row(result, "gpu-applied").code, "GPU_RESTART_REQUIRED");
  result = await offline({ ...context, gpuStatus: () => ({ ...base, softwareRenderer: true }) });
  assert.equal(row(result, "gpu-runtime").code, "GPU_SOFTWARE_FALLBACK");
  result = await offline({ ...context, gpuStatus: () => ({ ...base, configuredEnabled: false, appliedEnabled: false }) });
  assert.equal(row(result, "gpu-runtime").code, "GPU_DISABLE_FAILED");
  result = await offline({ ...context, gpuStatus: () => ({ ...base, configuredEnabled: false, appliedEnabled: false, hardwareEnabled: false, compositing: "disabled_software" }) });
  assert.equal(row(result, "gpu-runtime").status, "ok");
  assert.equal(row(result, "gpu-runtime").detailKey, "#L_DGV_GPU_SOFTWARE");
});

test("unknown GPU information is reported as unknown, not successful acceleration", async (t) => {
  const { context } = fixture(t);
  const result = await offline({ ...context, gpuStatus: () => ({ configuredEnabled: true, appliedEnabled: true, available: false }) });
  assert.equal(row(result, "gpu-runtime").code, "GPU_STATUS_UNKNOWN");
  assert.equal(row(result, "gpu-runtime").status, "warn");
});

test("updater errors and launch failures are visible; native module is checked once", async (t) => {
  const { context } = fixture(t);
  const result = await offline({ ...context, platform: "win32", nativeAvailable: false, nativeLoadError: "ABI mismatch", updateState: "error", launchAvailability: () => ({ ok: false, code: "SOURCE_SDK_NOT_INSTALLED" }) });
  assert.equal(row(result, "hlsr-updater").status, "fail");
  assert.equal(row(result, "hlsr-native").detail, "ABI mismatch");
  assert.equal(row(result, "steam-launch").code, "SOURCE_SDK_NOT_INSTALLED");
  assert.equal(result.checks.some((item) => item.id === "steam-native"), false);
});

test("write probes leave no files and download checks detect a limit mismatch", async (t) => {
  const { root, context } = fixture(t);
  fs.writeFileSync(path.join(root, "download-settings.json"), JSON.stringify({ speedLimit: 5 }));
  const before = fs.readdirSync(root);
  let result = await offline({ ...context, downloadLimitBps: () => 1024 * 1024 });
  assert.equal(row(result, "hlsr-userdata").status, "ok");
  assert.equal(row(result, "lib-writable").status, "ok");
  assert.deepEqual(fs.readdirSync(root), before);
  assert.equal(row(result, "downloads-applied").code, "DOWNLOAD_LIMIT_NOT_APPLIED");
  result = await offline({ ...context, downloadLimitBps: () => 5 * 1024 * 1024 });
  assert.equal(row(result, "downloads-applied").status, "ok");
  fs.writeFileSync(path.join(root, "download-settings.json"), "{broken");
  result = await offline(context);
  assert.equal(row(result, "downloads-limit").status, "fail");
});

test("diagnostics show precise playtime and do not label an offline statistics server as active", async (t) => {
  const { context } = fixture(t);
  const result = await offline({ ...context,
    profile: () => ({ signedIn: true, playSeconds: 127, launchCount: 3, gamesPlayed: 1 }),
    telemetryStatus: () => ({ enabled: true, backendConfigured: false }),
  });
  assert.deepEqual(row(result, "profile-playtime").detailArgs, [0, 2, 7, 3, 1]);
  assert.equal(row(result, "profile-telemetry").status, "skip");
});

for (const waiting of ["gpu", "dns", "request", "ownership"]) {
  test(`cancellation interrupts a pending ${waiting} check`, { timeout: 3000 }, async (t) => {
    const { context } = fixture(t);
    const pending = () => new Promise(() => {});
    const runner = new Runner({ context: {
      ...context,
      gpuStatus: waiting === "gpu" ? pending : () => null,
      ...(waiting === "ownership" ? { requiredSteamApps: () => [{ appId: 220, title: "Half-Life 2" }], steamOwnership: pending } : {}),
    }, lookup: waiting === "dns" ? () => {} : (_, callback) => callback(null), request: pending });
    const start = performance.now();
    const running = runner.run();
    setTimeout(() => runner.cancel(), 15);
    const result = await running;
    assert.equal(result.status, "cancelled");
    assert.ok(performance.now() - start < 1000);
    assert.equal(runner.running, false);
  });
}

function signedChannels(platform) {
  const keys = signing.generateKeyPair();
  const version = "0.3.17";
  const hash = "a".repeat(64), sha512 = Buffer.alloc(64).toString("base64");
  const launcher = {
    schemaVersion: 1, version,
    publishedAt: "2026-10-03T00:00:00Z", minimumSupportedVersion: "0.3.0", updateRequired: false,
    installer: { filename: platform === "linux" ? `HLSR-Setup-${version}-linux-x86_64.AppImage` : `HLSR-Setup-${version}.exe`, size: 1024, sha256: hash, sha512, urls: ["https://hlsr.pro/downloads/test"] },
  };
  launcher.signature = signing.sign(launcher, keys.privateKeyPem);
  const provider = { id: "primary", type: "https-direct", status: "permanent", url: "https://hlsr.pro/downloads/test" };
  const game = {
    schemaVersion: 1, gameId: "ghosting-renewed", channel: "stable", version: "1.0.0", publishedAt: "2026-10-03T00:00:00Z", minimumLauncherVersion: "0.3.0", enabled: true, updateRequired: false,
    fullArchive: { filename: "test.zip", size: 1024, sha256: hash, providers: [provider] },
    filesManifest: { filename: "files.json", version: "1.0.0", size: 512, sha256: hash, providers: [provider] },
  };
  game.signature = signing.sign(game, keys.privateKeyPem);
  const channelService = new ChannelService();
  channelService.configFor = () => ({ channel: "stable", requireSignature: true, publicKeys: [keys.publicKeyPem] });
  const proton = { tag_name: ProtonManager.RELEASE_TAG, assets: [{ name: ProtonManager.PINNED_ARCHIVE.name, size: ProtonManager.PINNED_ARCHIVE.size, browser_download_url: "https://github.com/test/archive" }] };
  return { keys, launcher, game, channelService, proton };
}

async function network(context, f, patch) {
  const urls = [];
  const runner = new Runner({ context, channelService: f.channelService, launcherPublicKeys: [f.keys.publicKeyPem], lookup: (_, callback) => callback(null), request: async (url) => {
    urls.push(url);
    let body = url.endsWith("stable.json") ? f.game : url.includes("launcher") ? f.launcher : url.includes("releases/tags") ? f.proton : {};
    if (url.endsWith("latest.yml")) body = "version: 0.3.17\nfiles:\n  - url: test.exe\n";
    const result = { status: 200, body: Buffer.from(typeof body === "string" ? body : JSON.stringify(body)) };
    return patch ? patch(url, result) : result;
  } });
  const result = await runner.run();
  return { ...result, urls };
}

for (const platform of ["linux", "win32"]) {
  test(`${platform} diagnostics verify the correct signed launcher and game channels`, async (t) => {
    const { context } = fixture(t);
    const f = signedChannels(platform);
    const result = await network({ ...context, platform }, f);
    assert.equal(row(result, "net-launcher-channel").status, "ok");
    assert.equal(row(result, "net-launcher-fallback").status, "ok");
    assert.equal(row(result, "net-game-channel").status, "ok");
    for (const url of endpoints.launcherFor(platform).channelUrls) assert.ok(result.urls.includes(url));
    const tampered = await network({ ...context, platform }, f, (url, response) => url.endsWith("stable.json") ? { ...response, body: Buffer.from(JSON.stringify({ ...f.game, version: "1.0.1" })) } : response);
    assert.equal(row(tampered, "net-game-channel").code, "SIGNATURE_INVALID");
  });
}

test("empty, truncated or redirected metadata cannot pass as a healthy channel", async (t) => {
  const { context } = fixture(t), f = signedChannels("linux");
  for (const patch of [{ body: null }, { body: Buffer.from("{}") }, { truncated: true }, { status: 302 }, { status: 202 }]) {
    const result = await network(context, f, (url, response) => url.endsWith("stable.json") ? { ...response, ...patch } : response);
    assert.equal(row(result, "net-game-channel").status, "fail");
  }
  const result = await network(context, f, (url, response) => url.endsWith("launcher-channel.json") ? { ...response, body: Buffer.from(JSON.stringify({ ...f.launcher, signature: null })) } : response);
  assert.equal(row(result, "net-launcher-channel").status, "fail");
});

test("malformed GitHub rate-limit data is unknown rather than healthy", async (t) => {
  const { context } = fixture(t), f = signedChannels("linux");
  const result = await network(context, f, (url, response) => url.includes("/rate_limit") ? { ...response, body: Buffer.from("{}") } : url === Runner.ENDPOINTS.githubReleases ? { ...response, status: 403 } : response);
  assert.equal(row(result, "net-rate-limit").status, "warn");
  assert.equal(row(result, "net-rate-limit").code, "CHECKSUM_METADATA_INVALID");
});

test("diagnostic row names and structured values are localized in both languages", () => {
  const source = fs.readFileSync(path.join(__dirname, "../src/main/diagnostics/DiagnosticsRunner.js"), "utf8");
  const ids = [...source.matchAll(/\b(?:check|probe)\("([a-z0-9-]+)"/g)].map((match) => "#L_DG_" + match[1].toUpperCase().replace(/-/g, "_"));
  const values = [...source.matchAll(/"(#L_DGV_[A-Z_]+)"/g)].map((match) => match[1]);
  for (const language of ["ru", "en"]) {
    const filename = path.join(__dirname, `../src/renderer/utils/locales/${language}.locale.js`);
    const code = require("@babel/core").transformFileSync(filename, {
    babelrc: false, configFile: false, presets: [[require.resolve("@babel/preset-env"), { targets: { node: "current" }, modules: "commonjs" }]],
  }).code;
    const module = { exports: {} };
    vm.runInNewContext(code, { module, exports: module.exports, require: createRequire(filename) });
    for (const key of [...ids, ...values]) assert.ok(module.exports.default[key], `${language}: missing ${key}`);
  }
});

test("отчёт берёт журнал запуска игры под Proton и тип графической сессии", (t) => {
  const exporter = require("../src/main/diagnostics/DiagnosticsExport");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-diagnostics-logs-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const logs = path.join(root, "logs");
  fs.mkdirSync(logs);
  fs.writeFileSync(path.join(logs, "main.log"), "[info] старт\n");
  fs.writeFileSync(path.join(logs, "proton-ghosting-renewed.log"), "# запуск\nerr: /home/alice/Games/HLSR/hl2.exe\n");
  fs.writeFileSync(path.join(logs, "proton-Bad_Name.log"), "не журнал игры\n");
  const collected = exporter.collect({ outDir: path.join(root, "out"), logsDir: logs, userName: "alice",
    env: { XDG_CURRENT_DESKTOP: "KDE", WAYLAND_DISPLAY: "wayland-0", DISPLAY: ":0", HLSR_TOKEN: "secret" } });
  const names = collected.files.map((file) => path.basename(file)).sort();
  assert.deepEqual(names, ["main.log", "manifest.json", "proton-ghosting-renewed.log"]);
  const text = fs.readFileSync(path.join(root, "out/proton-ghosting-renewed.log"), "utf8");
  assert.doesNotMatch(text, /alice/, "имя пользователя в путях скрыто");
  assert.deepEqual(collected.manifest.env, { XDG_CURRENT_DESKTOP: "KDE", WAYLAND_DISPLAY: "wayland-0", DISPLAY: ":0" });
});
