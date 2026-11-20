"use strict";

// Окно о краше (GameCrashModal) просит прислать текстовый лог краша, если он
// лежит рядом с дампом (Ghosting: ghosting_crash-<время>.txt и .dmp).
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const crashDumps = require("../src/main/game-delivery/crash-dumps");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-crash-report-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("лог краша: .txt рядом с дампом под тем же именем", (t) => {
  const dir = tempDir(t);
  const dump = path.join(dir, "ghosting_crash-2026-10-30-19-08-19-107332.dmp");
  const log = path.join(dir, "ghosting_crash-2026-10-30-19-08-19-107332.txt");
  fs.writeFileSync(dump, "MDMP");
  fs.writeFileSync(log, "EXCEPTION_ACCESS_VIOLATION in server.dll+0x1D5F8");
  assert.equal(crashDumps.crashLogOf(dump), log);
});

test("лог краша: нет .txt — null, и тогда присылают сам дамп", (t) => {
  const dir = tempDir(t);
  const dump = path.join(dir, "hl2.exe.1234.dmp");
  fs.writeFileSync(dump, "MDMP");
  assert.equal(crashDumps.crashLogOf(dump), null);
});

test("лог краша: только для дампов, папка с именем .txt не считается", (t) => {
  const dir = tempDir(t);
  const dump = path.join(dir, "crash.mdmp");
  fs.writeFileSync(dump, "MDMP");
  fs.mkdirSync(path.join(dir, "crash.txt"));
  assert.equal(crashDumps.crashLogOf(dump), null);
  assert.equal(crashDumps.crashLogOf(path.join(dir, "notes.log")), null);
  assert.equal(crashDumps.crashLogOf(null), null);
});

// Ghosting пакует .txt и .dmp сбоя в ghosting_crash-<время>.zip в папке мода:
// окно о краше просит этот архив, «Открыть папку» выделяет его.
function ghostingInstall(t) {
  const root = tempDir(t);
  const mod = path.join(root, "ghostingmod");
  fs.mkdirSync(mod);
  return { root, mod };
}

function writeAt(file, body, mtimeMs) {
  fs.writeFileSync(file, body);
  fs.utimesSync(file, mtimeMs / 1000, mtimeMs / 1000);
}

test("архив сбоя: самый новый ghosting_crash-*.zip после запуска", (t) => {
  const { root, mod } = ghostingInstall(t);
  const started = Date.now() - 60000;
  writeAt(path.join(mod, "ghosting_crash-2026-11-01-10-00-00-1.zip"), "PK old", started - 3600000);
  writeAt(path.join(mod, "ghosting_crash-2026-11-07-12-00-00-2.zip"), "PK first", started + 1000);
  writeAt(path.join(mod, "ghosting_crash-2026-11-07-12-05-00-3.zip"), "PK newest", started + 5000);
  writeAt(path.join(mod, "ghosting-demos-2026-11-07-1200.zip"), "PK demos", started + 9000);
  writeAt(path.join(mod, "ghosting_crash-2026-11-07-12-06-00-4.zip"), "", started + 9000);
  fs.mkdirSync(path.join(mod, "ghosting_crash-dir.zip"));
  assert.equal(crashDumps.crashArchiveOf(root, started, "ghosting-renewed", {}),
    path.join(mod, "ghosting_crash-2026-11-07-12-05-00-3.zip"));
});

test("архив сбоя: старый архив, другая игра или нет папки — null", (t) => {
  const { root, mod } = ghostingInstall(t);
  const started = Date.now();
  writeAt(path.join(mod, "ghosting_crash-2026-11-01-10-00-00-1.zip"), "PK old", started - 3600000);
  assert.equal(crashDumps.crashArchiveOf(root, started, "ghosting-renewed", {}), null);
  writeAt(path.join(mod, "ghosting_crash-2026-11-07-12-00-00-2.zip"), "PK new", started + 1000);
  assert.equal(crashDumps.crashArchiveOf(root, started, "hl2-4260", {}), null, "у HL2 своя папка мода");
  assert.equal(crashDumps.crashArchiveOf(null, started, "ghosting-renewed", {}), null);
});

test("архив сбоя: открывать можно только zip сбоя в папках игры", (t) => {
  const { root, mod } = ghostingInstall(t);
  const zip = path.join(mod, "ghosting_crash-2026-11-07-12-00-00-2.zip");
  fs.writeFileSync(zip, "PK");
  fs.writeFileSync(path.join(mod, "notes.zip"), "PK");
  assert.equal(crashDumps.isAllowedDumpPath(zip, root, {}), true);
  assert.equal(crashDumps.isAllowedDumpPath(path.join(mod, "notes.zip"), root, {}), false);
  const outside = path.join(tempDir(t), "ghosting_crash-x.zip");
  fs.writeFileSync(outside, "PK");
  assert.equal(crashDumps.isAllowedDumpPath(outside, root, {}), false);
});

test("окно о краше: архив сбоя в событии, «Открыть папку» выделяет его", async (t) => {
  const { root, mod } = ghostingInstall(t);
  const started = Date.now() - 60000;
  const base = path.join(mod, "ghosting_crash-2026-11-07-12-00-00-2");
  for (const ext of [".txt", ".dmp", ".zip"]) writeAt(base + ext, "data " + ext, started + 1000);

  // electron в обычном Node — строка пути; подменяем его на shell с записью вызовов.
  const shown = [];
  const electronPath = require.resolve("electron");
  const previous = require.cache[electronPath];
  require.cache[electronPath] = { id: electronPath, filename: electronPath, loaded: true, exports: { shell: { showItemInFolder: (file) => shown.push(file) } } };
  t.after(() => { if (previous) require.cache[electronPath] = previous; else delete require.cache[electronPath]; });
  const GameDelivery = require("../src/main/game-delivery");

  const emitted = [];
  const context = {
    dumpSnapshots: new Map([["ghosting-renewed", { installPath: root, snapshot: new Map(), startedAtMs: started }]]),
    lastCrash: new Map(),
    crashHintOf: () => null,
    emitTo: (channel, payload) => emitted.push([channel, payload]),
    notifications: null,
    window: null,
  };
  context.crashInfo = (id) => GameDelivery.prototype.crashInfo.call(context, id);
  context.resolveGameId = (id) => id;
  context.latestCrash = () => GameDelivery.prototype.latestCrash.call(context);
  t.mock.method(crashDumps, "waitForDump", async () => [{ path: base + ".dmp", size: 9, mtimeMs: started + 1000, zone: "game-mod" }]);

  await GameDelivery.prototype.handleCrash.call(context, { gameId: "ghosting-renewed", lastExit: { exitCode: 0xc0000005 } });
  const [channel, report] = emitted[0];
  assert.equal(channel, "game-crash-report");
  assert.equal(report.archive, true);
  assert.equal(report.file, base + ".zip");
  assert.equal(report.hasDump, true);

  assert.deepEqual(GameDelivery.prototype.openCrashDump.call(context, "ghosting-renewed"), { status: "ok" });
  assert.deepEqual(shown, [base + ".zip"]);

  // Без архива (другие игры) — как раньше: лог рядом с дампом.
  fs.rmSync(base + ".zip");
  context.dumpSnapshots.set("ghosting-renewed", { installPath: root, snapshot: new Map(), startedAtMs: started });
  emitted.length = 0;
  await GameDelivery.prototype.handleCrash.call(context, { gameId: "ghosting-renewed", lastExit: { exitCode: 1 } });
  assert.equal(emitted[0][1].archive, false);
  assert.equal(emitted[0][1].file, base + ".txt");
  GameDelivery.prototype.openCrashDump.call(context, "ghosting-renewed");
  assert.equal(shown[1], base + ".txt");
});
