"use strict";

// «Демо»: FFmpeg как скачиваемый инструмент (ToolService: sha256 архива,
// распаковка только ffmpeg.exe и ffprobe.exe, sha256 ffmpeg.exe, версия 9.0.2)
// и пробы кодировщиков. Полная установка — с локального сервера из zip
// (HLSR_DEMO_FFMPEG_ZIP — путь к ffmpeg-9.0.2-essentials_build.zip).
const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const ToolService = require("../src/main/game-delivery/ToolService");
const ArchiveService = require("../src/main/game-delivery/ArchiveService");
const toolRegistry = require("../src/main/game-delivery/tools");
const DemoTool = require("../src/main/demo-tool");
const ffmpeg = require("../src/main/demo-tool/ffmpeg");
const fx = require("./lib/demo-fixtures");

const manifest = toolRegistry.DEPENDENCIES.ffmpeg;
const zip = process.env.HLSR_DEMO_FFMPEG_ZIP;
const needZip = zip && fs.existsSync(zip) && process.platform === "win32"
  ? false
  : "ПРОПУЩЕНО: нет HLSR_DEMO_FFMPEG_ZIP (путь к ffmpeg-9.0.2-essentials_build.zip) или не Windows — установка FFmpeg не проверена";
const ff = fx.findFfmpeg();
const NO_FFMPEG = "ПРОПУЩЕНО: нет FFmpeg — задайте HLSR_DEMO_FFMPEG (папка bin FFmpeg 9.0.2)";

test("манифест FFmpeg: закреплённые версия, sha256, два источника; на панели инструментов его нет", () => {
  assert.equal(manifest.version, "9.0.2");
  assert.equal(manifest.archive.sha256, "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba");
  assert.equal(manifest.files["ffmpeg.exe"], "3256173f3f8bffd7df12227c68adf68025edb1832273a9530688a7bb1ed8edec");
  assert.equal(manifest.archive.size, 114768076);
  // Сначала GitHub (GyanD/codexffmpeg — зеркало сборок gyan.dev), запасной — сам gyan.dev.
  assert.deepEqual(manifest.archive.providers.map((provider) => new URL(provider.url).hostname), ["github.com", "www.gyan.dev"]);
  assert.deepEqual(manifest.extract, ["ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", "ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe"]);
  assert.equal(manifest.license, "GPL-3.0-or-later");
  assert.deepEqual(toolRegistry.ids(), ["livesplit"]);
  assert.equal(toolRegistry.manifestOf("ffmpeg"), null);
});

function serve(t, file) {
  return new Promise((resolve) => {
    const server = http.createServer((request, response) => {
      response.writeHead(200, { "Content-Length": fs.statSync(file).size });
      fs.createReadStream(file).pipe(response);
    });
    server.listen(0, "127.0.0.1", () => {
      t.after(() => server.close());
      resolve(`http://127.0.0.1:${server.address().port}/ffmpeg.zip`);
    });
  });
}

function toolFixture(t, url, overrides) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ffmpeg-tool-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));
  const paths = { root, tools: path.join(root, "Tools"), staging: path.join(root, ".staging"), downloads: path.join(root, ".downloads") };
  const local = JSON.parse(JSON.stringify(manifest));
  local.archive.providers = [{ id: "local", type: "https-direct", url }];
  Object.assign(local, overrides || {});
  const tools = new ToolService({ paths, registry: { ffmpeg: local }, archive: new ArchiveService() });
  return { root, paths, tools };
}

test("установка FFmpeg: только ffmpeg.exe и ffprobe.exe, sha256 сверены, версия 9.0.2, лицензия GPL", { timeout: 180000 }, async (t) => {
  if (needZip) return t.skip(needZip);
  const url = await serve(t, zip);
  const { tools, paths } = toolFixture(t, url);
  const percents = [];
  const tool = new DemoTool({ store: fx.memoryStore(), tools: () => tools, platform: "win32", production: true,
    jobsRoot: path.join(paths.root, "jobs"), emit: (channel, payload) => percents.push(payload.percent) });
  assert.equal((await tool.ffmpegStatus()).state, "not-installed");
  const installed = await tool.installFfmpeg();
  assert.equal(installed.status, "ok");
  assert.equal(installed.ffmpeg.state, "ready");
  assert.equal(installed.ffmpeg.source, "tool");
  assert.equal(installed.ffmpeg.version, "9.0.2-essentials_build-www.gyan.dev");
  assert.equal(installed.ffmpeg.license, "GPL");
  assert.equal(percents[percents.length - 1], 100);
  const dir = path.join(paths.tools, "ffmpeg");
  assert.deepEqual(fs.readdirSync(dir).sort(), ["ffmpeg.exe", "ffprobe.exe", "installed.json"]);
  assert.deepEqual(fs.readdirSync(paths.downloads), [], "архив удалён");
  assert.equal(JSON.parse(fs.readFileSync(path.join(dir, "installed.json"), "utf8")).version, "9.0.2");
});

test("установка FFmpeg: подмена ffmpeg.exe в архиве — CHECKSUM_MISMATCH, ничего не установлено", { timeout: 180000 }, async (t) => {
  if (needZip) return t.skip(needZip);
  const url = await serve(t, zip);
  const { tools, paths } = toolFixture(t, url, { files: { "ffmpeg.exe": "0".repeat(64) } });
  await assert.rejects(tools.install("ffmpeg"), { code: "CHECKSUM_MISMATCH" });
  assert.equal(fs.existsSync(path.join(paths.tools, "ffmpeg")), false);
  assert.equal(tools.status("ffmpeg").state, "not-installed");
});

test("установка FFmpeg: архив с другим sha256 — отказ ещё до распаковки", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ffmpeg-bad-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bad = path.join(root, "bad.zip");
  fs.writeFileSync(bad, Buffer.alloc(1024));
  const url = await serve(t, bad);
  const { tools } = toolFixture(t, url, { archive: Object.assign({}, manifest.archive, { size: 1024, providers: [{ id: "local", url }] }) });
  await assert.rejects(tools.install("ffmpeg"), { code: "CHECKSUM_MISMATCH" });
});

test("статус FFmpeg: Linux — из PATH без скачивания; не та версия инструмента — FFMPEG_VERSION_MISMATCH", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ffmpeg-status-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const linux = new DemoTool({ store: fx.memoryStore(), platform: "linux", production: true, jobsRoot: path.join(root, "jobs") });
  linux.versionOf = async () => { throw Object.assign(new Error("нет"), { code: "FFMPEG_MISSING" }); };
  const missing = await linux.ffmpegStatus();
  assert.deepEqual([missing.state, missing.code, missing.installable], ["not-installed", "FFMPEG_NOT_IN_PATH", false]);
  await assert.rejects(linux.installFfmpeg(), { code: "FFMPEG_INSTALL_UNSUPPORTED" });
  linux.versionOf = async () => ({ version: "6.1.1", pinned: false, license: "GPL" });
  const fromPath = await linux.ffmpegStatus();
  assert.deepEqual([fromPath.state, fromPath.source, fromPath.version], ["ready", "path", "6.1.1"]);

  const toolDir = path.join(root, "Tools", "ffmpeg");
  fs.mkdirSync(toolDir, { recursive: true });
  fs.writeFileSync(path.join(toolDir, "ffmpeg.exe"), "");
  const fakeTools = { readInstalled: () => ({ version: "9.0.2" }), installPathOf: () => toolDir };
  const win = new DemoTool({ store: fx.memoryStore(), platform: "win32", production: true, tools: () => fakeTools, jobsRoot: path.join(root, "jobs") });
  win.versionOf = async () => ({ version: "8.0", pinned: false, license: "GPL" });
  const wrong = await win.ffmpegStatus();
  assert.deepEqual([wrong.state, wrong.code], ["error", "FFMPEG_VERSION_MISMATCH"]);
  await assert.rejects(win.requireFfmpeg(), { code: "FFMPEG_VERSION_MISMATCH" });
});

test("пробы: GPU в порядке NVENC → AMF → QSV, результат на сессию, CPU без пробы", async () => {
  const calls = [];
  const probe = new ffmpeg.EncoderProbe({
    exe: "ffmpeg",
    listFn: async () => new Set(["libx264", "h264_nvenc", "h264_amf", "h264_qsv", "aac"]),
    runFn: async (exe, args) => {
      const encoder = args[args.indexOf("-c:v") + 1];
      calls.push(`${encoder} ${args[args.indexOf("-i") + 1]}`);
      if (encoder !== "h264_qsv") throw Object.assign(new Error("нет устройства"), { code: "FFMPEG_FAILED" });
      return {};
    },
  });
  const summary = await probe.summary();
  assert.deepEqual([summary.gpu, summary.cpu, summary.auto], ["h264_qsv", "libx264", "h264_qsv"]);
  await probe.summary();
  assert.deepEqual(calls, [
    "h264_nvenc testsrc2=size=1920x1080:rate=60",
    "h264_amf testsrc2=size=1920x1080:rate=60",
    "h264_qsv testsrc2=size=1920x1080:rate=60",
  ]);
  assert.ok(ffmpeg.probeArgs("h264_qsv", 3840, 2160, 120).includes("nv12"));

  const none = new ffmpeg.EncoderProbe({ exe: "ffmpeg", listFn: async () => new Set(["libx264"]), runFn: async () => assert.fail("без пробы") });
  assert.deepEqual(await none.summary().then((value) => [value.gpu, value.auto]), [null, "libx264"]);
});

test("версия настоящего FFmpeg: первая строка содержит 9.0.2, сборка GPL", async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const info = await ffmpeg.version(ff.ffmpeg);
  assert.ok(info.firstLine.startsWith("ffmpeg version "), info.firstLine);
  assert.equal(info.pinned, info.firstLine.includes(" 9.0.2"));
  assert.equal(info.license, "GPL");
  const names = await ffmpeg.listEncoders(ff.ffmpeg);
  assert.ok(names.has("libx264") && names.has("aac"));
});

test("общие каналы tool-*: FFmpeg не запускается, не останавливается, не удаляется и не открывается как инструмент панели", async () => {
  const electronPath = require.resolve("electron");
  const handlers = new Map();
  require.cache[electronPath] = { id: electronPath, filename: electronPath, loaded: true,
    exports: { ipcMain: { handle: (channel, fn) => handlers.set(channel, fn), on() {} } } };
  const GameDelivery = require("../src/main/game-delivery");
  const delivery = new GameDelivery({});
  let touched = 0;
  delivery.tools = new Proxy({}, { get: () => () => { touched++; return { state: "ready" }; } });
  delivery.initialize();
  for (const channel of ["tool-launch", "tool-stop", "tool-remove", "tool-open-folder", "tool-install"])
    assert.deepEqual(await handlers.get(channel)({}, "ffmpeg"), { status: "error", error: { code: "TOOL_UNKNOWN" } }, channel);
  assert.equal(await handlers.get("tool-cancel")({}, "ffmpeg"), false);
  assert.equal((await handlers.get("tool-status")({}, "ffmpeg")).state, "unknown");
  assert.equal(touched, 0, "ToolService не вызывался для ffmpeg");
  assert.deepEqual((await handlers.get("tool-list")()).map((card) => card.title), ["LiveSplit"]);
});

test("установка FFmpeg посреди задания — JOB_BUSY; задание во время установки — FFMPEG_INSTALLING", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ffmpeg-busy-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const tool = new DemoTool({ store: fx.memoryStore(), platform: "win32", production: true, tools: () => ({}), jobsRoot: path.join(root, "jobs") });
  tool.job = { id: "x", kind: "export", controller: new AbortController() };
  await assert.rejects(tool.installFfmpeg(), { code: "JOB_BUSY" });
  tool.job = null;
  tool.install = { toolId: "ffmpeg", percent: 40 };
  await assert.rejects(tool.startExport("any", {}), { code: "FFMPEG_INSTALLING" });
  assert.equal((await tool.ffmpegStatus()).state, "installing");
  // Скачивание bxt-rs — не FFmpeg: его статус «installing», FFmpeg — нет.
  tool.install = { toolId: "bxt-rs", percent: 10 };
  assert.equal(tool.bxtrsStatus().state, "installing");
  assert.notEqual((await tool.ffmpegStatus()).state, "installing");
  await assert.rejects(tool.installFfmpeg(), { code: "ALREADY_RUNNING" });
});
