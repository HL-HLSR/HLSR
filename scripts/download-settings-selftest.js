"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const http = require("node:http");
const crypto = require("node:crypto");
const vm = require("node:vm");
const { createRequire } = require("node:module");
const { performance } = require("node:perf_hooks");
const DownloadRateLimiter = require("../src/main/game-delivery/DownloadRateLimiter");
const DownloadSettings = require("../src/main/game-delivery/DownloadSettings");
const GameDownloadManager = require("../src/main/game-delivery/GameDownloadManager");
const ToolService = require("../src/main/game-delivery/ToolService");
const { RepairMirrorClient } = require("../src/main/game-delivery/repair-mirror");
const LocalHttpProvider = require("../src/main/game-delivery/providers/LocalHttpProvider");
const { resolveLibrary } = require("../src/main/game-delivery/paths");
const httpClient = require("../src/main/game-delivery/http-client");

async function fixture(t, size, broken = false) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-download-limit-"));
  const payload = crypto.randomBytes(size);
  const ranges = [];
  const server = http.createServer((request, response) => {
    const offset = Number((request.headers.range || "").match(/bytes=(\d+)-/)?.[1] || 0);
    if (offset) ranges.push(offset);
    response.writeHead(offset ? 206 : 200, {
      "content-length": payload.length - offset,
      ...(offset ? { "content-range": `bytes ${offset}-${payload.length - 1}/${payload.length}` } : {}),
    });
    if (broken) {
      response.write(payload.subarray(0, 1024));
      setTimeout(() => response.destroy(), 20);
    } else response.end(payload.subarray(offset));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    fs.rmSync(root, { recursive: true, force: true });
  });
  const url = `http://127.0.0.1:${server.address().port}/archive`;
  const paths = resolveLibrary(root);
  const sha256 = crypto.createHash("sha256").update(payload).digest("hex");
  const request = {
    gameId: "test-game", filename: "test.zip", expectedSize: size, expectedSha256: sha256,
    provider: new LocalHttpProvider({ id: "local", type: "local-http", url }),
  };
  const manager = (rateLimiter) => new GameDownloadManager({
    paths, rateLimiter, maxAttempts: 1, freeSpaceProvider: () => Infinity,
  });
  return { root, paths, payload, sha256, url, request, manager, ranges };
}

test("download limit paces real HTTP bytes without changing the archive", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 128 * 1024);
  const start = performance.now();
  const result = await f.manager(new DownloadRateLimiter(128 * 1024)).start(f.request);
  assert.ok(performance.now() - start >= 900, "128 KB at 128 KB/s must take about a second");
  assert.equal(result.status, "completed");
  assert.deepEqual(fs.readFileSync(result.filePath), f.payload);
});

test("game and LiveSplit downloads share one bandwidth limit", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 256 * 1024);
  const limiter = new DownloadRateLimiter(512 * 1024);
  const tools = new ToolService({ paths: f.paths, rateLimiter: limiter });
  const target = path.join(f.root, "livesplit.zip");
  const start = performance.now();
  const [result] = await Promise.all([
    f.manager(limiter).start(f.request),
    tools.downloadFrom(f.url, { size: f.payload.length, sha256: f.sha256 }, target),
  ]);
  assert.ok(performance.now() - start >= 900, "two 256 KB files share 512 KB/s");
  assert.deepEqual(fs.readFileSync(result.filePath), f.payload);
  assert.deepEqual(fs.readFileSync(target), f.payload);
});

test("removing a limit wakes an active HTTP download immediately", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 512 * 1024);
  const limiter = new DownloadRateLimiter(1024);
  const manager = f.manager(limiter);
  manager.on("state", (state) => {
    if (state.state === "downloading") setTimeout(() => limiter.setLimit(0), 40);
  });
  const start = performance.now();
  const result = await manager.start(f.request);
  assert.ok(performance.now() - start < 1500, "a cleared limit must not keep the old timer");
  assert.deepEqual(fs.readFileSync(result.filePath), f.payload);
});

test("pause and Range resume retain the exact archive under a limit", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 512 * 1024);
  const limiter = new DownloadRateLimiter(256 * 1024);
  const manager = f.manager(limiter);
  manager.once("progress", () => manager.pause());
  const paused = await manager.start(f.request);
  assert.equal(paused.status, "paused");
  assert.ok(paused.bytesDownloaded > 0);
  limiter.setLimit(0);
  const result = await manager.start(f.request);
  assert.ok(f.ranges.length > 0, "resume must request the saved byte range");
  assert.deepEqual(fs.readFileSync(result.filePath), f.payload);
});

test("changing a positive limit takes effect on the current stream", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 256 * 1024);
  const limiter = new DownloadRateLimiter(1);
  const manager = f.manager(limiter);
  manager.on("state", (state) => {
    if (state.state === "downloading") setTimeout(() => limiter.setLimit(256 * 1024), 30);
  });
  const start = performance.now();
  const result = await manager.start(f.request);
  const elapsed = performance.now() - start;
  assert.ok(elapsed >= 900 && elapsed < 3000, `expected about one second, got ${elapsed} ms`);
  assert.deepEqual(fs.readFileSync(result.filePath), f.payload);
});

test("cancellation interrupts a limiter wait and removes partial files", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 128 * 1024);
  const manager = f.manager(new DownloadRateLimiter(1));
  manager.on("state", (state) => {
    if (state.state === "downloading") setTimeout(() => manager.cancel(), 30);
  });
  const start = performance.now();
  assert.equal((await manager.start(f.request)).status, "cancelled");
  assert.ok(performance.now() - start < 1500);
  assert.equal(fs.existsSync(manager.partPaths(f.request.gameId, f.request.filename).part), false);
});

test("repair mirror transfers obey the same limit and validate SHA-256", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 64 * 1024);
  const repair = new RepairMirrorClient({ rateLimiter: new DownloadRateLimiter(64 * 1024), maxAttempts: 1 });
  const target = path.join(f.root, "test.bin");
  const start = performance.now();
  await repair.fetchOne(f.url, { path: "test.bin", size: f.payload.length, sha256: f.sha256 }, "sha256-2", target);
  assert.ok(performance.now() - start >= 900);
  assert.deepEqual(fs.readFileSync(target), f.payload);
  const wrong = { path: "test.bin", size: f.payload.length, sha256: "0".repeat(64) };
  await assert.rejects(repair.fetchOne(f.url, wrong, "sha256-2", target + ".bad"), { code: "CHECKSUM_MISMATCH" });
  assert.equal(fs.existsSync(target + ".bad") || fs.existsSync(target + ".bad.part"), false, "битый файл не остаётся");
});

test("a broken limited connection remains a network error", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 128 * 1024, true);
  await assert.rejects(f.manager(new DownloadRateLimiter(1024 * 1024)).start(f.request), { code: "NETWORK_ERROR" });
});

test("limited response collection still rejects oversized bodies", { timeout: 10000 }, async (t) => {
  const f = await fixture(t, 64 * 1024);
  const response = await httpClient.request(f.url);
  await assert.rejects(httpClient.readBody(response, 1024, new DownloadRateLimiter(1024 * 1024)), { code: "HTTP_ERROR" });
});

test("download preference survives restart and rejects invalid IPC values", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-download-prefs-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = path.join(root, "download-settings.json");
  const settings = new DownloadSettings({ file });
  assert.equal(settings.snapshot().initialized, false);
  assert.equal(settings.save(5).ok, true);
  assert.equal(new DownloadSettings({ file }).snapshot().speedLimit, 5);
  for (const value of [-1, Infinity, NaN, "5", null, {}, 3]) {
    assert.equal(settings.save(value).ok, false);
    assert.equal(new DownloadSettings({ file }).snapshot().speedLimit, 5);
  }
  fs.writeFileSync(file, "{broken");
  const broken = new DownloadSettings({ file });
  assert.equal(broken.save(10).ok, false);
  assert.equal(fs.readFileSync(file, "utf8"), "{broken");
  fs.writeFileSync(file, JSON.stringify({ speedLimit: 5 }));
  assert.equal(broken.save(1).ok, true, "saving can be retried after the file becomes readable");
});

test("old launcher preferences cannot disable build updates or inject an invalid limit", () => {
  const filename = path.resolve(__dirname, "../src/renderer/components/Launcher/launcher-prefs.js");
  const code = require("@babel/core").transformFileSync(filename, {
    babelrc: false, configFile: false, presets: [[require.resolve("@babel/preset-env"), { targets: { node: "current" }, modules: "commonjs" }]],
  }).code;
  let stored = JSON.stringify({ autoUpdate: false, uiScale: 110, telemetry: false, speedLimit: 5, animations: false });
  const module = { exports: {} };
  vm.runInNewContext(code, {
    module, exports: module.exports, require: createRequire(filename),
    window: { hlsrElectron: { defaultLibraryPath: "/tmp/HLSR" } },
    localStorage: { getItem: () => stored, setItem: (_, value) => { stored = value; } },
  });
  const prefs = module.exports.default;
  const loaded = prefs.load();
  assert.equal(loaded.autoUpdate, undefined);
  assert.equal(loaded.uiScale, undefined);
  assert.equal(loaded.telemetry, undefined);
  assert.equal(loaded.speedLimit, 5);
  assert.equal(loaded.animations, false);
  prefs.save({ autoUpdate: false, speedLimit: 10 });
  assert.equal(JSON.parse(stored).autoUpdate, undefined);
  assert.equal(JSON.parse(stored).speedLimit, 10);
  for (const invalid of ["5", -1, null]) {
    stored = JSON.stringify({ speedLimit: invalid });
    assert.equal(prefs.load().speedLimit, 0);
  }
  for (const invalid of ["null", "12", "[]", "\"text\"", "{broken"]) {
    stored = invalid;
    assert.equal(prefs.load().speedLimit, 0);
  }
});
