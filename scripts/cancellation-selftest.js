"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs"), os = require("os"), path = require("path");
const { EventEmitter, once } = require("events");
const { PassThrough } = require("stream");
const { openInstallerWindow } = require("../src/main/launcher-update/installer-window");
const ToolService = require("../src/main/game-delivery/ToolService");
const GameDownloadManager = require("../src/main/game-delivery/GameDownloadManager");
const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
const ProtonManager = require("../src/main/linux/ProtonManager");

function temporary(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cancel-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
}
function untilAbort(signal) {
  return new Promise((resolve, reject) => {
    if (signal.aborted) return reject(signal.reason);
    signal.addEventListener("abort", () => reject(signal.reason), { once: true });
  });
}
function installerFixture(backend) {
  const handlers = new Map();
  let options;
  const window = new EventEmitter();
  window.webContents = new EventEmitter();
  Object.assign(window.webContents, { send() {}, setZoomFactor() {}, setVisualZoomLevelLimits() {}, setWindowOpenHandler() {} });
  Object.assign(window, {
    removeMenu() {}, loadFile() {}, minimize() { this.minimized = true; },
    destroy() { this.destroyed = true; this.emit("closed"); },
    close() { const event = { preventDefault() { this.prevented = true; } }; this.emit("close", event); if (!event.prevented) this.destroy(); },
  });
  const electron = {
    BrowserWindow: function (value) { options = value; return window; },
    ipcMain: { handle: (name, fn) => handlers.set(name, fn), removeHandler: (name) => handlers.delete(name) },
    dialog: { showOpenDialog: async () => ({ canceled: false, filePaths: ["/tmp/chosen-launcher"] }) },
  };
  openInstallerWindow(electron, { assets: "/tmp/assets", state: { phase: "idle", directory: "/tmp/default" }, backend });
  const call = (name, ...args) => handlers.get(name)({ sender: window.webContents }, ...args);
  return { window, call, options, handlers };
}

test("installer: cancellation leaves the window reusable, folder and titlebar controls work", async () => {
  let runs = 0;
  const f = installerFixture({
    install: async (desktop, ctx) => {
      runs++;
      ctx.send({ phase: "installing" });
      if (runs === 1) await untilAbort(ctx.signal);
      else ctx.send({ phase: "done" });
      return true;
    },
    selectDirectory: (directory, ctx) => ctx.send({ directory, phase: "idle" }),
  });
  assert.equal(f.options.width, 640); assert.equal(f.options.height, 560);
  assert.equal(f.options.resizable, false); assert.equal(f.options.maximizable, false); assert.equal(f.options.fullscreenable, false);
  await f.call("installer-select-directory");
  assert.equal((await f.call("installer-get-state")).directory, "/tmp/chosen-launcher");
  const pending = f.call("installer-install", true);
  assert.equal(await f.call("installer-select-directory"), false);
  assert.equal(await f.call("installer-cancel"), true);
  assert.equal(await pending, false);
  assert.equal((await f.call("installer-get-state")).phase, "cancelled");
  assert.equal(f.window.destroyed, undefined);
  assert.equal(await f.call("installer-install", false), true);
  assert.equal((await f.call("installer-get-state")).phase, "done");
  await f.call("installer-window", "minimize"); assert.equal(f.window.minimized, true);
  await f.call("installer-window", "close"); assert.equal(f.window.destroyed, true);
  assert.equal(f.handlers.size, 0);
});

test("installer: close during preparation waits for abort cleanup", async () => {
  let cleaned = false;
  const f = installerFixture({ install: async (desktop, ctx) => {
    try { await untilAbort(ctx.signal); } finally { cleaned = true; }
  } });
  const pending = f.call("installer-install", false);
  await f.call("installer-window", "close");
  assert.equal(f.window.destroyed, undefined);
  await pending;
  assert.equal(cleaned, true); assert.equal(f.window.destroyed, true);
});

test("installer: commit cannot be cancelled halfway; keyboard zoom is blocked", async () => {
  let finish, signal;
  const f = installerFixture({ install: async (desktop, ctx) => {
    signal = ctx.signal; ctx.commit();
    await new Promise(resolve => { finish = resolve; });
    ctx.send({ phase: "done" });
  } });
  const pending = f.call("installer-install", false);
  assert.equal(await f.call("installer-cancel"), false);
  await f.call("installer-window", "close"); assert.equal(signal.aborted, false);
  finish(); await pending; assert.equal(f.window.destroyed, true);
  const event = { preventDefault() { this.prevented = true; } };
  f.window.webContents.emit("before-input-event", event, { key: "=", control: true });
  assert.equal(event.prevented, true);
});

function toolFixture(t, extra = {}) {
  const root = temporary(t), paths = { root, downloads: path.join(root, "Downloads"), staging: path.join(root, "staging"), tools: path.join(root, "Tools") };
  const registry = { timer: { version: "2", executable: "timer.exe", archive: { filename: "timer.zip", size: 100, providers: [{ id: "one", url: "https://example.test/timer" }, { id: "two", url: "https://example.test/mirror" }] } } };
  const service = new ToolService({ paths, registry, ...extra });
  return { service, paths };
}
test("tools: cancelling a stream deletes partial files and skips mirror retries", async (t) => {
  const response = new PassThrough(); response.statusCode = 200;
  let attempts = 0;
  const f = toolFixture(t, { httpClient: { request: async () => { attempts++; return response; } } });
  const ready = new Promise(resolve => { f.progressReady = resolve; });
  const pending = f.service.install("timer", () => f.progressReady());
  const rejected = assert.rejects(pending, error => error.code === "CANCELLED");
  response.write(Buffer.alloc(10)); await ready;
  assert.equal(f.service.cancel("other"), false);
  assert.equal(f.service.cancel("timer"), true);
  await rejected;
  assert.equal(attempts, 1); assert.equal(response.destroyed, true);
  assert.deepEqual(fs.readdirSync(f.paths.downloads), []);
  assert.equal(f.service.busyWith, null);
});
test("tools: cancelling extraction preserves the installed version and supports retry", async (t) => {
  let rejectExtract, started;
  const ready = new Promise(resolve => { started = resolve; });
  const f = toolFixture(t, { archive: { extract: () => {
    const promise = new Promise((resolve, reject) => { rejectExtract = reject; });
    started(); return { promise, cancel: () => rejectExtract(new Error("stopped")) };
  } } });
  f.service.download = async (manifest, target, onProgress, signal) => { signal.throwIfAborted(); fs.writeFileSync(target, "archive"); };
  const target = path.join(f.paths.tools, "timer"); fs.mkdirSync(target, { recursive: true }); fs.writeFileSync(path.join(target, "timer.exe"), "old");
  const pending = f.service.install("timer"); const rejected = assert.rejects(pending, error => error.code === "CANCELLED");
  await ready; f.service.cancel("timer"); await rejected;
  assert.equal(fs.readFileSync(path.join(target, "timer.exe"), "utf8"), "old");
  assert.deepEqual(fs.readdirSync(f.paths.staging), []);
  f.service.archive = { extract: (archive, staging) => { fs.writeFileSync(path.join(staging, "timer.exe"), "new"); return { promise: Promise.resolve() }; } };
  assert.equal((await f.service.install("timer")).status, "installed");
  assert.equal(fs.readFileSync(path.join(target, "timer.exe"), "utf8"), "new");
});
test("games: cancelling during retry backoff is prompt and deletes a paused download", async (t) => {
  const root = temporary(t), manager = new GameDownloadManager({ paths: { downloads: root }, backoffBaseMs: 30000, freeSpaceProvider: () => null });
  manager.attempt = async () => { const error = new Error("offline"); error.code = "NETWORK_ERROR"; throw error; };
  const retrying = once(manager, "state").then(() => manager.cancel());
  const start = Date.now();
  const pending = manager.start({ gameId: "game", filename: "game.zip", expectedSize: 100, expectedSha256: "a".repeat(64), provider: { id: "one" } });
  await retrying; assert.equal((await pending).status, "cancelled"); assert.ok(Date.now() - start < 2000);
  const files = manager.partPaths("game", "game.zip"); fs.writeFileSync(files.part, "partial"); manager.writeState(files.state, { gameId: "game", filename: "game.zip" });
  fs.writeFileSync(path.join(files.dir, "keep.zip"), "complete");
  assert.equal(manager.discardPaused("game"), true);
  assert.deepEqual(fs.readdirSync(files.dir), ["keep.zip"]);
});
test("games: cancelling a paused game does not interrupt another active download", () => {
  let activeCancels = 0, discarded;
  const state = { paused: { state: "paused" }, active: { state: "downloading" }, other: { state: "ready" } };
  const context = {
    slot: { gameId: "active" },
    downloadManager: { discardPaused: id => { discarded = id; }, cancel: () => { activeCancels++; return true; } },
    installService: { cancel: () => false },
    statusOf: id => state[id], setStatus: (id, patch) => Object.assign(state[id], patch),
    setOperation() {}, refresh() {},
  };
  assert.equal(GameDeliveryService.prototype.cancel.call(context, "paused"), true);
  assert.equal(discarded, "paused"); assert.equal(activeCancels, 0);
  assert.equal(state.paused.state, "cancelled");
  assert.equal(GameDeliveryService.prototype.cancel.call(context, "other"), false);
  assert.equal(GameDeliveryService.prototype.cancel.call(context, "active"), true);
  assert.equal(activeCancels, 1);
});
test("Proton: cancellation at metadata and download stages clears busy state and staging", async (t) => {
  for (const stage of ["checking", "downloading"]) {
    const root = temporary(t); let started;
    const ready = new Promise(resolve => { started = resolve; });
    const release = { tag_name: ProtonManager.RELEASE_TAG, assets: [{ ...ProtonManager.PINNED_ARCHIVE, browser_download_url: "https://github.com/test" }] };
    const response = new PassThrough();
    const manager = new ProtonManager({ dataDir: root, home: root,
      readText: async (url, limit, signal) => { if (stage === "checking") { started(); await untilAbort(signal); } return JSON.stringify(release); },
      request: async () => { started(); return response; },
    });
    const pending = manager.ensureReference(); await ready;
    assert.equal(manager.cancel(), true); assert.equal(await pending, null);
    assert.equal(manager.state.phase, "idle"); assert.equal(manager.state.error, null); assert.equal(manager.pending, null);
    if (fs.existsSync(manager.root)) assert.deepEqual(fs.readdirSync(manager.root), []);
    if (stage === "downloading") assert.equal(response.destroyed, true);
  }
});
