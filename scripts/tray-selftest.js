"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const os = require("os");
const fs = require("fs");
const path = require("path");

const TrayController = require("../src/main/tray/TrayController");
const GameProcessService = require("../src/main/game-delivery/GameProcessService");

function fakeWindow() {
  const calls = [];
  return {
    calls,
    isDestroyed: () => false,
    isMinimized: () => false,
    hide: () => calls.push("hide"),
    show: () => calls.push("show"),
    focus: () => calls.push("focus"),
    restore: () => calls.push("restore"),
  };
}

function fakeElectron() {
  class Tray {
    setToolTip() {}
    setContextMenu() {}
    on() {}
    destroy() {}
  }
  return { Tray, Menu: { buildFromTemplate: () => ({}) }, nativeImage: null };
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("Трей: окно не возвращается, пока игра снова запущена", async () => {
  const window = fakeWindow();
  let running = true;
  const tray = new TrayController({ window, electron: fakeElectron(), iconPath: "x.png", settleMs: 20, isGameRunning: () => running });
  tray.onGameStarted();
  assert.deepEqual(window.calls, ["hide"]);
  tray.onGameFinished();
  await wait(40);
  assert.deepEqual(window.calls, ["hide"], "игра перезапустилась — окно в трее");
  running = false;
  tray.onGameFinished();
  await wait(40);
  assert.deepEqual(window.calls, ["hide", "show", "focus"]);
  tray.dispose();
});

test("Трей: окно, которое не пряталось, после игры фокус не забирает", async () => {
  const window = fakeWindow();
  const tray = new TrayController({ window, electron: fakeElectron(), iconPath: "x.png", settleMs: 10 });
  tray.setEnabled(false);
  tray.onGameStarted();
  tray.onGameFinished();
  await wait(30);
  assert.deepEqual(window.calls, []);
  tray.dispose();
});

test("Процесс: перезапуск GoldSrc — та же игра, без события finished", () => {
  const state = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-proc-"));
  const install = path.join(state, "game");
  const exe = path.join(install, "hl.exe");
  let alive = new Set([100]);
  const inspector = {
    isAlive: (pid) => alive.has(pid),
    samePath: (a, b) => path.resolve(a) === path.resolve(b),
    findByImageName: () => [...alive].map((pid) => ({ pid, executablePath: exe, startedAt: new Date().toISOString() })),
  };
  const service = new GameProcessService({ paths: { state }, inspector, pollIntervalMs: 1000000 });
  const finished = [];
  service.on("finished", (event) => finished.push(event));
  service.registerStartedProcess("hl1-won", { pid: 100, executablePath: exe, installPath: install });

  alive = new Set([200]);
  service.handleExit("hl1-won", null, null, null, true);
  assert.equal(finished.length, 0);
  assert.equal(service.isRunning("hl1-won"), true);
  assert.equal(service.tracked.get("hl1-won").record.pid, 200);

  alive = new Set();
  service.handleExit("hl1-won", null, null, null, true);
  assert.equal(finished.length, 1);
  fs.rmSync(state, { recursive: true, force: true });
});

test("Процесс: запись в общей библиотеке не подхватывает и не останавливает чужой процесс", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-proc-"));
  const paths = { state: path.join(root, ".hlsr", "state"), games: path.join(root, "Games") };
  fs.mkdirSync(paths.state, { recursive: true });
  const started = "2026-11-07T10:00:00.000Z";
  const foreign = path.join(root, "Users", "owner", "app.exe");
  const exeOf = (gameId) => path.join(paths.games, gameId, "hl.exe");
  const processes = {
    300: { executablePath: foreign, startedAt: started },
    400: { executablePath: exeOf("hl2-4260"), startedAt: started },
    500: { executablePath: exeOf("hl2-2707"), startedAt: "2026-11-07T12:00:00.000Z" },
    600: { executablePath: exeOf("hl1-won"), startedAt: started },
  };
  const inspector = Object.assign({}, require("../src/main/game-delivery/process-inspect"), {
    isAlive: (pid) => !!processes[pid],
    inspect: (pid) => (processes[pid] ? Object.assign({ pid }, processes[pid]) : null),
  });
  const write = (gameId, record) => fs.writeFileSync(path.join(paths.state, `${gameId}.process.json`),
    JSON.stringify(Object.assign({ version: 1, gameId, state: "running", installPath: path.dirname(exeOf(gameId)) }, record)));

  // Чужой процесс, папка «игры» подставлена в самой записи.
  write("ghosting-renewed", { pid: 300, executablePath: foreign, installPath: path.dirname(foreign) });
  // Образ игры, но без времени старта и с чужим временем.
  write("hl2-4260", { pid: 400, executablePath: exeOf("hl2-4260") });
  write("hl2-2707", { pid: 500, executablePath: exeOf("hl2-2707"), startedAt: started });
  // Своя запись: папка этой игры и то же время старта.
  write("hl1-won", { pid: 600, executablePath: exeOf("hl1-won"), startedAt: started });

  const stops = [];
  const stopper = { graceful: async (pid) => { stops.push(pid); delete processes[pid]; return { ok: true, output: "" }; } };
  const service = new GameProcessService({ paths, inspector, stopper, pollIntervalMs: 1000000 });
  try {
    const adopted = service.reconcileAfterLauncherRestart().filter((item) => item.adopted).map((item) => item.gameId);
    assert.deepEqual(adopted, ["hl1-won"]);
    assert.deepEqual(await service.requestStop("ghosting-renewed"), { status: "not-running" });
    assert.deepEqual(await service.requestStop("hl1-won"), { status: "stopped" });
    assert.deepEqual(stops, [600]);
  } finally {
    service.dispose();
    fs.rmSync(root, { recursive: true, force: true });
  }
});
