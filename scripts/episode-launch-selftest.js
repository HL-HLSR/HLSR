"use strict";

// Запуск эпизода Ghosting: hl2.exe с -game ep2, конфиг Ghosting в эпизод до spawn и обратно —
// на выходе игры, на старте лаунчера и перед следующим запуском Ghosting.
// node --test scripts/episode-launch-selftest.js

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { EventEmitter } = require("events");

// index.js доставки и discord-rpc берут ipcMain из electron при загрузке: свой — до их require.
const ipcHandlers = new Map();
const electronPath = require.resolve("electron");
require.cache[electronPath] = {
  id: electronPath,
  filename: electronPath,
  loaded: true,
  exports: { ipcMain: { handle: (channel, fn) => ipcHandlers.set(channel, fn), on() {} } },
};

const LaunchService = require("../src/main/game-delivery/LaunchService");
const LinuxLaunchService = require("../src/main/linux/LaunchService");
const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
const GameProcessService = require("../src/main/game-delivery/GameProcessService");
const GameDelivery = require("../src/main/game-delivery");
const qaLaunch = require("../src/main/game-delivery/qa-launch");
const manifests = require("../src/main/game-delivery/manifests");
const episodes = require("../src/main/game-delivery/episodes");
const presenceModel = require("../src/main/discord-rpc/presence-model");
const { DiscordRichPresenceBridge } = require("../src/main/discord-rpc");
const { gameInstallPath } = require("../src/main/game-delivery/paths");

const GAME = "ghosting-renewed";
const quiet = { info() {}, warn() {}, error() {} };
const GAMEINFO = '"GameInfo"\n{\n\tFileSystem\n\t{\n\t\tSteamAppId 243730\n\t}\n}\n';
const full = (root, relative) => path.join(root, ...relative.split("/"));
const read = (root, relative) => (fs.existsSync(full(root, relative)) ? fs.readFileSync(full(root, relative), "utf8") : null);

function write(root, relative, data) {
  fs.mkdirSync(path.dirname(full(root, relative)), { recursive: true });
  fs.writeFileSync(full(root, relative), data);
}

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-episode-launch-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Установленная Ghosting: cfg игрока и стоковый chapter1.cfg; Episode One и (если ep2) Episode Two.
function ghosting(root, { ep2 = true } = {}) {
  write(root, "hl2.exe", "never spawned");
  write(root, "steam_appid.txt", "243730\n\0");
  write(root, "ghostingmod/gameinfo.txt", GAMEINFO);
  write(root, "ghostingmod/cfg/autoexec.cfg", "// файл игрока\nexec tc_n\n");
  write(root, "ghostingmod/cfg/config.cfg", "bind w +forward\n");
  write(root, "ghostingmod/cfg/tc_n.cfg", "// свой cfg игрока\n");
  write(root, "ghostingmod/cfg/chapter1.cfg", "map d1_trainstation_01\n");
  const ids = ep2 ? ["ep1", "ep2"] : ["ep1"];
  for (const id of ids) {
    const dir = episodes.EPISODES[id].dir;
    write(root, `${dir}/gameinfo.txt`, GAMEINFO);
    write(root, `${dir}/${episodes.MARKER}`, JSON.stringify({ version: "1.0.0" }));
    write(root, `${dir}/cfg/chapter1.cfg`, `map ${id}_first\n`);
  }
}

const WATCHED = [
  "ep2/cfg/autoexec.cfg",
  "ep2/cfg/config.cfg",
  "ep2/cfg/tc_n.cfg",
  "ep2/cfg/chapter1.cfg",
  "ghostingmod/cfg/autoexec.cfg",
  "ghostingmod/cfg/config.cfg",
];

// LaunchService с заглушками Steam, процесса и spawn: в момент spawn запоминаются cfg на диске.
function launcher(t, options = {}) {
  const root = tempDir(t);
  ghosting(root, options);
  const calls = { spawned: [], registered: [], starting: 0, gameinfo: [] };
  const service = new LaunchService({
    platform: "win32",
    env: { PATH: "x" },
    delivery: {
      processService: { isRunning: () => false, markStarting() { calls.starting += 1; }, cancelStarting() {} },
      installedOf: () => ({ installPath: root, version: "1.0.13" }),
      isBusy: () => false,
      filesManifestFor: () => ({ files: [{ path: "ghostingmod/cfg/chapter1.cfg" }, { path: "ghostingmod/cfg/config.cfg" }] }),
      registerStartedProcess: (gameId, info) => {
        calls.registered.push(Object.assign({ gameId }, info));
        return { gameId, state: "running", episode: info.episode };
      },
    },
    launchRegistry: { [GAME]: manifests.LAUNCH_CONFIG[GAME], "hl2-4260": manifests.LAUNCH_CONFIG["hl2-4260"] },
    settingsStore: { get: () => options.settings || null },
    spawnFn: (file, argv, spawnOptions) => {
      calls.spawned.push({ file, argv, cwd: spawnOptions.cwd, files: Object.fromEntries(WATCHED.map((relative) => [relative, read(root, relative)])) });
      const child = new EventEmitter();
      child.pid = 4242;
      return child;
    },
    inspector: { isAlive: () => true, inspect: () => null, samePath: (a, b) => a === b },
    earlyExitWindowMs: 0,
    steamLocator: { describeSteamOnly: () => ({ ok: true, steam: { pid: 1 } }) },
    logger: quiet,
  });
  const resolve = service.resolveGameFiles.bind(service);
  service.resolveGameFiles = (installPath, config) => {
    const files = resolve(installPath, config);
    calls.gameinfo.push(files.gameinfo);
    return files;
  };
  return { root, service, calls };
}

test("эпизод: hl2.exe с -game ep2, gameinfo.txt эпизода, конфиг Ghosting в эпизоде уже на spawn", async (t) => {
  const { root, service, calls } = launcher(t);
  const result = await service.launch(GAME, { episode: "ep2" });

  assert.equal(result.status, "started");
  assert.equal(calls.spawned.length, 1);
  const { file, argv, cwd, files } = calls.spawned[0];
  assert.equal(file, path.join(root, "hl2.exe"));
  assert.equal(cwd, root);
  assert.equal(argv[argv.indexOf("-game") + 1], "ep2");
  assert.ok(!argv.includes("ghostingmod"), argv.join(" "));
  assert.deepEqual(calls.gameinfo, [path.join(root, "ep2", "gameinfo.txt")]);

  assert.ok(files["ep2/cfg/autoexec.cfg"], "файл игрока в эпизоде");
  assert.equal(files["ep2/cfg/autoexec.cfg"], read(root, "ghostingmod/cfg/autoexec.cfg"), "файл игрока");
  assert.equal(files["ep2/cfg/config.cfg"], "bind w +forward\n", "бинды");
  assert.equal(files["ep2/cfg/tc_n.cfg"], "// свой cfg игрока\n", "свой cfg игрока");
  assert.equal(files["ep2/cfg/chapter1.cfg"], "map ep2_first\n", "стоковый cfg Ghosting в эпизод не идёт");

  assert.equal(calls.registered[0].episode, "ep2");
  assert.deepEqual(episodes.pendingSync(root), ["ep2"], "отметка для возврата config.cfg");
});

test("эпизод: свой -game в параметрах запуска игрока не перебивает каталог эпизода", async (t) => {
  for (const launchArgs of ["-game ghostingmod", "-novid +game=ghostingmod", "-GAME ghostingmod"]) {
    const { service, calls } = launcher(t, { settings: { launchArgs } });
    await service.launch(GAME, { episode: "ep2" });
    const { argv } = calls.spawned[0];
    assert.deepEqual(argv.filter((arg) => /^[-+/]+game/i.test(arg)), ["-game"], launchArgs);
    assert.equal(argv[argv.indexOf("-game") + 1], "ep2", launchArgs);
    assert.ok(!argv.includes("ghostingmod"), launchArgs);
  }
});

test("эпизод не установлен — NOT_INSTALLED, без spawn и без «запускается»", async (t) => {
  const { service, calls } = launcher(t, { ep2: false });
  await assert.rejects(service.launch(GAME, { episode: "ep2" }), (error) => error.code === "NOT_INSTALLED");
  assert.equal(calls.spawned.length, 0);
  assert.equal(calls.starting, 0);
});

test("чужой эпизод или эпизод не у Ghosting — LAUNCH_CONFIG_INVALID", async (t) => {
  const { service, calls } = launcher(t);
  await assert.rejects(service.launch(GAME, { episode: "ep3" }), (error) => error.code === "LAUNCH_CONFIG_INVALID");
  await assert.rejects(service.launch(GAME, { episode: "__proto__" }), (error) => error.code === "LAUNCH_CONFIG_INVALID");
  await assert.rejects(service.launch("hl2-4260", { episode: "ep1" }), (error) => error.code === "LAUNCH_CONFIG_INVALID");
  await assert.rejects(service.launch(GAME, { episode: ["ep1"] }), (error) => error.code === "LAUNCH_CONFIG_INVALID");
  assert.equal(calls.spawned.length, 0);
  assert.equal(episodes.isEpisode(["ep1"]), false, "массив из IPC — не эпизод");
});

test("Linux: эпизод — LAUNCH_CONFIG_INVALID до «запускается», а не сама Ghosting", async () => {
  let starting = 0;
  const service = new LinuxLaunchService({
    delivery: { processService: { isRunning: () => false, markStarting() { starting += 1; } }, isBusy: () => false },
    steamLocator: {},
  });
  await assert.rejects(service.launch(GAME, { episode: "ep1" }), (error) => error.code === "LAUNCH_CONFIG_INVALID" && /Linux/.test(error.message));
  assert.equal(starting, 0);
});

// GameDelivery (index.js) с заглушками вокруг launch: что дошло до LaunchService.
function gameDelivery() {
  const delivery = new GameDelivery({});
  const launches = [];
  let qaStarted = 0;
  delivery.service = {
    isGameRunning: () => false,
    installedOf: () => assert.fail("QA-запуск дошёл до installedOf"),
    processService: { markStarting() { qaStarted += 1; } },
  };
  delivery.telemetryGameStarting = () => {};
  delivery.telemetryGameLaunchFailed = () => {};
  delivery.ensurePracticeSaves = () => {};
  delivery.launchService = {
    launch: async (gameId, options) => {
      launches.push({ gameId, options });
      return { status: "started" };
    },
  };
  return { delivery, launches, qaStarted: () => qaStarted };
}

test("IPC game-process-launch: { episode } доходит до LaunchService, чужое значение — отказ", async () => {
  const { delivery, launches } = gameDelivery();
  delivery.initialize();
  const launch = ipcHandlers.get("game-process-launch");

  assert.equal((await launch({}, GAME, { episode: "ep2" })).status, "started");
  assert.equal((await launch({}, GAME)).status, "started");
  for (const episode of [["ep1"], "ep3", { id: "ep1" }]) {
    const answer = await launch({}, GAME, { episode });
    assert.equal(answer.status, "error", JSON.stringify(episode));
    assert.equal(answer.error.code, "LAUNCH_CONFIG_INVALID", JSON.stringify(episode));
  }
  assert.deepEqual(launches, [
    { gameId: GAME, options: { episode: "ep2" } },
    { gameId: GAME, options: {} },
  ]);
});

test("QA-запуск: эпизод — отказ до тестового процесса", async (t) => {
  t.mock.method(qaLaunch, "isEnabled", () => true);
  const { delivery, launches, qaStarted } = gameDelivery();
  await assert.rejects(delivery.launch(GAME, { episode: "ep1" }), (error) => error.code === "LAUNCH_CONFIG_INVALID");
  assert.equal(qaStarted(), 0);
  assert.equal(launches.length, 0);
});

test("запуск Ghosting: висящий config.cfg эпизода сначала возвращается в ghostingmod", async (t) => {
  const { root, service, calls } = launcher(t);
  episodes.syncConfigIn(root, "ep2");
  write(root, "ep2/cfg/config.cfg", "bind w +forward\nbind e +use\n");

  await service.launch(GAME);

  const { argv, files } = calls.spawned[0];
  assert.equal(argv[argv.indexOf("-game") + 1], "ghostingmod");
  assert.equal(files["ghostingmod/cfg/config.cfg"], "bind w +forward\nbind e +use\n");
  assert.deepEqual(episodes.pendingSync(root), []);
  assert.equal(calls.registered[0].episode, null);
});

// Временная библиотека с установленной Ghosting и настоящими GameDeliveryService/GameProcessService.
function deliveryService(t, libraryPath) {
  const manifest = Object.assign({}, manifests.MANIFESTS[GAME]);
  delete manifest.buildMetadata;
  const service = new GameDeliveryService({
    libraryPath,
    launcherVersion: "0.4.9",
    manifests: { [GAME]: manifest },
    channelService: null,
    originUpdates: null,
    updateCheckIntervalMs: 0,
    logger: quiet,
  });
  t.after(() => service.dispose());
  return service;
}

function delivery(t) {
  const libraryPath = path.join(tempDir(t), "library");
  const service = deliveryService(t, libraryPath);
  const installPath = gameInstallPath(service.paths, GAME);
  ghosting(installPath);
  service.installService.writeInstalled(
    { gameId: GAME, manifest: { version: "1.0.13" }, installPath, publishedAt: "2026-11-12T00:00:00.000Z" },
    "a".repeat(64)
  );
  return { service, installPath, libraryPath };
}

const started = (installPath, extra) =>
  Object.assign({ pid: process.pid, executablePath: path.join(installPath, "hl2.exe"), child: new EventEmitter() }, extra);

test("выход эпизода: изменённый config.cfg возвращается в ghostingmod/cfg; эпизод в состоянии процесса", (t) => {
  const { service, installPath } = delivery(t);
  episodes.syncConfigIn(installPath, "ep2");
  write(installPath, "ep2/cfg/config.cfg", "bind w +forward\nbind e +use\n");

  const info = started(installPath, { episode: "ep2" });
  service.registerStartedProcess(GAME, info);
  assert.equal(service.processState(GAME).episode, "ep2");
  assert.equal(JSON.parse(fs.readFileSync(service.processService.statePath(GAME), "utf8")).episode, "ep2", "в файле состояния процесса");
  assert.equal(read(installPath, "ghostingmod/cfg/config.cfg"), "bind w +forward\n", "пока игра идёт — не трогаем");

  info.child.emit("exit", 0, null);

  assert.equal(read(installPath, "ghostingmod/cfg/config.cfg"), "bind w +forward\nbind e +use\n");
  assert.deepEqual(episodes.pendingSync(installPath), []);
});

test("старт лаунчера: висящий .hlsr-sync.json возвращается, только если игра не запущена", (t) => {
  const { service, installPath, libraryPath } = delivery(t);
  episodes.syncConfigIn(installPath, "ep1");
  write(installPath, "episodic/cfg/config.cfg", "bind q lastinv\n");

  const info = started(installPath);
  service.registerStartedProcess(GAME, info);
  service.returnEpisodeConfigs(GAME);
  assert.deepEqual(episodes.pendingSync(installPath), ["ep1"], "Ghosting запущена — ждём");
  info.child.emit("exit", 0, null);

  // Лаунчер упал и запущен снова: новая служба над той же библиотекой.
  deliveryService(t, libraryPath).reconcileProcesses();

  assert.equal(read(installPath, "ghostingmod/cfg/config.cfg"), "bind q lastinv\n");
  assert.deepEqual(episodes.pendingSync(installPath), []);
});

// Файл состояния процесса, как его оставил прошлый лаунчер; inspector — «процесс жив и тот же».
function persistedRecord(service, installPath, episode) {
  const record = {
    version: 1,
    gameId: GAME,
    state: "running",
    pid: 777,
    executablePath: path.join(installPath, "hl2.exe"),
    installPath,
    startedAt: "2026-11-14T10:00:00.000Z",
    processKey: null,
    episode,
  };
  fs.mkdirSync(service.paths.state, { recursive: true });
  fs.writeFileSync(service.processService.statePath(GAME), JSON.stringify(record));
  service.processService.inspector = {
    isAlive: () => true,
    inspect: () => ({ executablePath: record.executablePath, startedAt: record.startedAt }),
    samePath: (a, b) => a.toLowerCase() === b.toLowerCase(),
    sameStart: () => true,
  };
}

test("перезапуск лаунчера при идущем эпизоде: процесс подхвачен, на выходе config.cfg вернулся", (t) => {
  const { service, installPath } = delivery(t);
  episodes.syncConfigIn(installPath, "ep2");
  write(installPath, "ep2/cfg/config.cfg", "bind e +use\n");
  persistedRecord(service, installPath, "ep2");

  const [adopted] = service.reconcileProcesses();
  assert.equal(adopted.adopted, true);
  assert.equal(service.processState(GAME).episode, "ep2");
  assert.deepEqual(episodes.pendingSync(installPath), ["ep2"], "игра идёт — не возвращаем");

  service.processService.markExited(GAME, 777);
  assert.equal(read(installPath, "ghostingmod/cfg/config.cfg"), "bind e +use\n");
  assert.deepEqual(episodes.pendingSync(installPath), []);
});

test("эпизод из файла состояния процесса и из registerStartedProcess — только известный", (t) => {
  for (const bad of ["x", {}, ["ep2"]]) {
    const { service, installPath } = delivery(t);
    persistedRecord(service, installPath, bad);
    const [adopted] = service.reconcileProcesses();
    assert.equal(adopted.adopted, true, JSON.stringify(bad));
    assert.equal(service.processState(GAME).episode, null, JSON.stringify(bad));
    service.processService.markExited(GAME, 777);
  }
  const { service, installPath } = delivery(t);
  service.registerStartedProcess(GAME, started(installPath, { episode: "x" }));
  assert.equal(service.processState(GAME).episode, null);
});

test("Discord: эпизод из GameProcessService доходит до presence сигналом running", (t) => {
  const dir = tempDir(t);
  const processService = new GameProcessService({ paths: { state: dir }, logger: quiet });
  t.after(() => processService.dispose());
  const delivery = new GameDelivery({});
  delivery.service = { processService, installService: { readInstalled: () => null }, checkForUpdatesIfDue() {} };
  delivery.attachProcessEvents();
  const bridge = new DiscordRichPresenceBridge({ userDataPath: dir, createClient: () => ({}) });
  const sent = [];
  bridge.service = { setPresence: (state) => sent.push(state), connected: false };
  assert.ok(bridge.attachDelivery(delivery));
  bridge.onSignal({ type: "selected", gameId: GAME });
  const details = () => presenceModel.toActivity(sent[sent.length - 1]).details;

  const child = new EventEmitter();
  processService.registerStartedProcess(GAME, { pid: process.pid, executablePath: path.join(dir, "hl2.exe"), installPath: dir, child, episode: "ep2" });
  assert.equal(details(), "HL2 Ghosting: Episode Two");
  child.emit("exit", 0, null);
  assert.equal(details(), "Half-Life 2: Ghosting Mod: Renewed");
});
