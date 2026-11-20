"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { EventEmitter } = require("events");

const manifests = require("../src/main/game-delivery/manifests");
const ManifestService = require("../src/main/game-delivery/ManifestService");
const mutablePaths = require("../src/main/game-delivery/mutable-paths");
const endpoints = require("../src/shared/delivery-endpoints");
const availability = require("../src/shared/game-availability");
const ProtonRunner = require("../src/main/linux/ProtonRunner");
const LinuxLaunchService = require("../src/main/linux/LaunchService");
const rinputMenuFix = require("../src/main/linux/rinput-menu-fix");
const rinputMenuFixWin = require("../src/main/game-delivery/rinput-menu-fix-win");
const WindowsLaunchService = require("../src/main/game-delivery/LaunchService");
const depCompat = require("../src/main/game-delivery/dep-compat");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-hl1-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function writeFile(file, content = "", mode) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
  if (mode) fs.chmodSync(file, mode);
}

function fakeInstall(root, appId = "220") {
  writeFile(path.join(root, "Half-Life/hl.exe"), "MZ");
  writeFile(path.join(root, "Half-Life/valve_WON/liblist.gam"), 'game "Half-Life"\n');
  writeFile(path.join(root, "Half-Life/steam_appid.txt"), appId);
  writeFile(path.join(root, "Bunnymod XT/Injector.exe"), "MZ");
  return root;
}

const config = manifests.LAUNCH_CONFIG["hl1-won"];

test("HL1: сборка WON зарегистрирована и доступна в интерфейсе", () => {
  assert.equal(manifests.forLauncherId(70).gameId, "hl1-won");
  assert.equal(manifests.launcherIdOf("hl1-won"), "70");
  assert.ok(availability.isGameAvailable("70"));
  assert.deepEqual(config.requiresOwnedAppIds, [70], "без лицензии Half-Life сборка не ставится");
  assert.equal(config.engine, "goldsrc");
  assert.ok(manifests.requiredOwnedApps().some((app) => app.appId === 70));
});

test("HL1: манифест проходит проверку и качается только с закреплённого хоста", () => {
  const manifest = manifests.MANIFESTS["hl1-won"];
  const check = new ManifestService({}).assertInstallable(manifest, {
    launcherVersion: "0.3.16",
    allowedProviderTypes: ["https-direct"],
  });
  for (const provider of check.providers)
    assert.ok(endpoints.ALLOWED_DOWNLOAD_HOSTS.includes(new URL(provider.url).hostname));

  const files = new Set(manifests.FILES_MANIFESTS["hl1-won"]["1.0.0"].files.map((file) => file.path));
  for (const required of manifest.filesManifest.requiredFiles) assert.ok(files.has(required), required);
  for (const relative of [config.executable, config.injector, config.steamAppIdFile])
    assert.ok(files.has(relative), relative);
});

test("HL1: сейвы, конфиги и служебные файлы игры не считаются повреждением", () => {
  for (const relative of [
    "Half-Life/valve_WON/SAVE/quick.sav",
    "Half-Life/valve_WON/config.cfg",
    "Half-Life/valve_WON/userconfig.cfg",
    "Half-Life/valve_WON/demoheader.dmf",
    "Half-Life/errorlogs/errorlog00001.txt",
    "Half-Life/steam_appid.txt",
  ])
    assert.ok(mutablePaths.matchesMutable(relative), relative);
  assert.ok(!mutablePaths.matchesMutable("Half-Life/hl.exe"));
  assert.ok(!mutablePaths.matchesMutable("Half-Life/hw.dll"));
});

test("HL1 Linux: hl.exe идёт через инжектор, а Proton получает мост к Steam", (t) => {
  const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
  const steamCalls = [];
  const service = new LinuxLaunchService({
    protonRunner: {},
    steamLocator: {
      locateSteam: () => ({ installPath: "/home/user/.local/share/Steam" }),
      assertSteamRunning: (steam) => { steamCalls.push(steam); return { pid: 1 }; },
    },
  });

  const plan = service.goldSrcLaunch({ installPath: root }, config, { launchArgs: "-w 1280 -h 960" });

  assert.equal(plan.executable, path.join(root, "Bunnymod XT/Injector.exe"));
  assert.equal(plan.argv[0], ProtonRunner.windowsPathOf(path.join(root, "Half-Life/hl.exe")));
  assert.deepEqual(plan.argv.slice(1), [...config.arguments, "-w", "1280", "-h", "960"]);
  assert.ok(!plan.argv.includes("-insecure"), "переключатели Source не попадают в HL1");
  assert.equal(plan.options.cwd, path.join(root, "Half-Life"));
  assert.equal(plan.options.env.BXT_DISABLE_VSYNC, "1");
  assert.equal(plan.options.env.PROTON_ENABLE_WAYLAND, "0");
  assert.equal(plan.options.env.__GL_SYNC_TO_VBLANK, "0");
  assert.deepEqual(plan.options.steamBridge, {
    appId: 70,
    steamPath: "/home/user/.local/share/Steam",
    installPath: root,
  });
  assert.equal(steamCalls.length, 1, "без запущенного Steam игра не стартует");
  assert.equal(fs.readFileSync(path.join(root, "Half-Life/steam_appid.txt"), "utf8"), "70\n");

  const forbidden = service.goldSrcLaunch({ installPath: root }, config, { launchArgs: "-game cstrike" });
  assert.deepEqual(forbidden.argv.slice(1), config.arguments, "игрок не подменяет мод через -game");
});

test("HL1 Linux: параметры Proton игрока доходят до запуска", (t) => {
  const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
  const service = new LinuxLaunchService({
    protonRunner: {},
    steamLocator: { locateSteam: () => ({ installPath: "/steam" }), assertSteamRunning: () => ({ pid: 1 }) },
  });
  const plan = service.goldSrcLaunch({ installPath: root }, config, {
    protonOptions: "PROTON_USE_WINED3D=1 __GL_SYNC_TO_VBLANK=1 mangohud %command% -nojoy",
  });
  assert.equal(plan.options.env.PROTON_USE_WINED3D, "1");
  assert.equal(plan.options.env.__GL_SYNC_TO_VBLANK, "1", "переменная игрока важнее значения лаунчера, как в Steam");
  assert.equal(plan.options.env.BXT_DISABLE_VSYNC, "1");
  assert.deepEqual(plan.options.wrapper, ["mangohud"]);
  assert.equal(plan.argv.at(-1), "-nojoy");

  const broken = service.goldSrcLaunch({ installPath: root }, config, { protonOptions: "WINEPREFIX=/x mangohud" });
  assert.deepEqual(broken.options.wrapper, [], "строка с защищённой переменной не применяется");
});

test("HL1 Linux: BXT пишет лог в папку логов лаунчера, старый лог удаляется", (t) => {
  const dir = tempDir(t);
  const root = fakeInstall(path.join(dir, "hl1-won"));
  const logsDir = path.join(dir, "logs");
  fs.mkdirSync(logsDir);
  fs.writeFileSync(path.join(logsDir, "bxt.log"), "прошлый запуск\n");
  const steamLocator = {
    locateSteam: () => ({ installPath: "/steam" }),
    assertSteamRunning: () => ({ pid: 1 }),
  };

  const withLogs = new LinuxLaunchService({ protonRunner: {}, steamLocator, logsDir });
  const plan = withLogs.goldSrcLaunch({ installPath: root }, config, null);
  assert.equal(plan.options.env.BXT_LOGFILE, ProtonRunner.windowsPathOf(path.join(logsDir, "bxt.log")));
  assert.ok(!fs.existsSync(path.join(logsDir, "bxt.log")), "BXT дописывает в конец, поэтому старый лог убран");

  const withoutLogs = new LinuxLaunchService({ protonRunner: {}, steamLocator });
  assert.equal(withoutLogs.goldSrcLaunch({ installPath: root }, config, null).options.env.BXT_LOGFILE, undefined);
});

test("HL1: недоступная папка логов не мешает запуску, а только отключает лог BXT", () => {
  const goldsrc = require("../src/main/game-delivery/goldsrc-launch");
  const warnings = [];
  const brokenFs = { mkdirSync() { throw new Error("EACCES"); }, rmSync() {} };
  const logger = { info() {}, warn: (m) => warnings.push(m) };
  assert.equal(goldsrc.prepareBxtLog("/nope", brokenFs, logger), null);
  assert.match(warnings[0], /лог BXT не подготовлен: EACCES/);
  assert.equal(goldsrc.prepareBxtLog(null, brokenFs, logger), null);
});

test("HL1 Linux: без Steam или без инжектора запуск отклоняется понятной ошибкой", (t) => {
  const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
  const { DeliveryError } = require("../src/main/game-delivery/errors");
  const offline = new LinuxLaunchService({
    protonRunner: {},
    steamLocator: {
      locateSteam: () => ({ installPath: "/steam" }),
      assertSteamRunning: () => { throw new DeliveryError("STEAM_NOT_RUNNING", "Steam не запущен"); },
    },
  });
  assert.throws(() => offline.goldSrcLaunch({ installPath: root }, config, null), { code: "STEAM_NOT_RUNNING" });

  fs.rmSync(path.join(root, "Bunnymod XT/Injector.exe"));
  assert.throws(() => offline.goldSrcLaunch({ installPath: root }, config, null), { code: "GAME_EXECUTABLE_MISSING" });
});

test("Proton: мост к Steam запускает Proton через Steam Runtime без umu", async (t) => {
  const root = tempDir(t);
  const proton = path.join(root, "proton-test");
  writeFile(path.join(proton, "proton"), "#!/bin/sh\n", 0o755);
  writeFile(path.join(proton, "toolmanifest.vdf"), '"manifest"\n{\n  "require_tool_appid" "4183110"\n}\n');
  const umu = path.join(root, "resources/umu/umu-run");
  writeFile(umu, "#!/bin/sh\n", 0o755);
  const dataHome = path.join(root, "data");
  const entryPoint = path.join(dataHome, "umu/steamrt4/_v2-entry-point");
  writeFile(entryPoint, "#!/bin/sh\n", 0o755);
  const injector = path.join(root, "game/Bunnymod XT/Injector.exe");
  writeFile(injector, "MZ");

  const previous = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = dataHome;
  t.after(() => {
    if (previous === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previous;
  });

  const calls = [];
  const runner = new ProtonRunner({
    manager: { selected: () => ({ path: proton, name: "test" }) },
    dataDir: root,
    resourcesPath: path.join(root, "resources"),
    pythonRuntime: { prepareEnvironment: async (env) => env },
    userNamespaces: { ensure: async () => {} },
    spawnFn: (command, args, options) => {
      calls.push({ command, args, options });
      const child = new EventEmitter();
      child.pid = 4242;
      setImmediate(() => child.emit("spawn"));
      return child;
    },
  });

  const result = await runner.launch("hl1-won", injector, ["Z:\\game\\hl.exe", "-steam"], {
    skipFonts: true,
    env: { BXT_DISABLE_VSYNC: "1" },
    steamBridge: { appId: 70, steamPath: "/steam", installPath: path.join(root, "game") },
  });

  const call = calls[0];
  assert.equal(call.command, entryPoint);
  assert.equal(result.command, entryPoint);
  assert.deepEqual(call.args, ["--verb=run", "--", path.join(proton, "proton"), "run", injector, "Z:\\game\\hl.exe", "-steam"]);
  const env = call.options.env;
  for (const key of ["UMU_ID", "GAMEID", "STORE", "PROTON_VERB", "WINEPREFIX"])
    assert.equal(env[key], undefined, `${key} отключил бы мост к Steam`);
  assert.equal(env.STEAM_COMPAT_DATA_PATH, runner.prefixFor("hl1-won"), "префикс тот же, что у umu");
  assert.equal(env.STEAM_COMPAT_CLIENT_INSTALL_PATH, "/steam");
  assert.equal(env.STEAM_COMPAT_INSTALL_PATH, path.join(root, "game"));
  assert.equal(env.SteamAppId, "70");
  assert.equal(env.SteamGameId, "70");
  assert.equal(env.STEAM_COMPAT_TOOL_PATHS, `${proton}:${path.dirname(entryPoint)}`);
  assert.equal(env.BXT_DISABLE_VSYNC, "1");
  assert.equal(call.options.cwd, path.dirname(injector));

  fs.rmSync(entryPoint);
  await assert.rejects(
    runner.launch("hl1-won", injector, [], { skipFonts: true, steamBridge: { appId: 70, steamPath: "/steam" } }),
    /Steam Runtime steamrt4 не найден/
  );
});

function injectedService(found, options = {}) {
  return new WindowsLaunchService({
    delivery: {},
    injectedGameTimeoutMs: options.timeoutMs || 2000,
    injectedGamePollMs: 5,
    inspector: {
      findByImageName: () => found(),
      samePath: (a, b) => path.resolve(a) === path.resolve(b),
      isAlive: () => true,
    },
    steamLocator: {},
  });
}

test("HL1 Windows: после инжектора отслеживается сам hl.exe", async () => {
  const executable = path.resolve("/games/hl1-won/Half-Life/hl.exe");
  const spawnedAt = Date.now();
  let polls = 0;
  const service = injectedService(() => {
    polls += 1;
    if (polls < 3) return [];
    return [
      { pid: 10, executablePath: executable, startedAt: new Date(spawnedAt - 60000).toISOString() },
      { pid: 11, executablePath: path.resolve("/other/hl.exe"), startedAt: new Date(spawnedAt).toISOString() },
      { pid: 12, executablePath: executable, startedAt: new Date(spawnedAt + 100).toISOString() },
    ];
  });
  const child = new EventEmitter();
  setImmediate(() => child.emit("exit", 0));

  const started = await service.waitForInjectedGame(child, executable, spawnedAt);
  assert.equal(started.pid, 12, "старый hl.exe и чужая копия игнорируются");
});

test("HL1 Windows: ошибка инжектора не превращается в зависший запуск", async () => {
  const executable = path.resolve("/games/hl1-won/Half-Life/hl.exe");

  const failed = injectedService(() => []);
  const crashed = new EventEmitter();
  setImmediate(() => crashed.emit("exit", 3));
  await assert.rejects(failed.waitForInjectedGame(crashed, executable, Date.now()), /кодом 3/);

  const timeout = injectedService(() => [], { timeoutMs: 50 });
  await assert.rejects(timeout.waitForInjectedGame(new EventEmitter(), executable, Date.now()), /за отведённое время/);
});

function fakeReg(initial) {
  const calls = [];
  let data = initial;
  const run = (args) => {
    calls.push(args);
    if (args[0] === "query") {
      if (data == null) return { status: 1, stdout: "", stderr: "ERROR: не найдено" };
      return { status: 0, stdout: `\r\n${depCompat.COMPAT_LAYERS}\r\n    ${args[3]}    REG_SZ    ${data}\r\n\r\n` };
    }
    if (args[0] === "add") {
      data = args[args.indexOf("/d") + 1];
      return { status: 0, stdout: "" };
    }
    return { status: 1 };
  };
  return { run, calls, get data() { return data; } };
}

test("HL1 Windows: DEP снимается с hl.exe слоем совместимости в HKCU", () => {
  const exe = "D:\\HLSR\\Half-Life\\Games\\hl1-won\\Half-Life\\hl.exe";

  const fresh = fakeReg(null);
  assert.equal(depCompat.ensureExemption(exe, { run: fresh.run }), "added");
  assert.equal(fresh.data, "~ DisableNXShowUI");
  const add = fresh.calls.find((args) => args[0] === "add");
  assert.ok(add[1].startsWith("HKCU\\"), "без прав администратора, только для пользователя");
  assert.deepEqual(add.slice(2, 4), ["/v", exe]);

  const admin = fakeReg("~ RUNASADMIN WINXPSP3");
  assert.equal(depCompat.ensureExemption(exe, { run: admin.run }), "added");
  assert.equal(admin.data, "~ RUNASADMIN WINXPSP3 DisableNXShowUI", "флаги из свойств exe сохраняются");

  const present = fakeReg("~ DisableNXShowUI");
  assert.equal(depCompat.ensureExemption(exe, { run: present.run }), "present");
  assert.ok(!present.calls.some((args) => args[0] === "add"), "повторно не пишется");

  const denied = { run: (args) => (args[0] === "add" ? { status: 1, stderr: "Отказано в доступе" } : { status: 1 }) };
  assert.equal(depCompat.ensureExemption(exe, denied), "failed");
});

test("HL1 Windows: подсказка о DEP только при AlwaysOn или незаписанном исключении", () => {
  assert.equal(depCompat.parsePolicy("3\r\n"), 3);
  assert.equal(depCompat.parsePolicy("ошибка"), null);

  assert.equal(depCompat.crashHint({ policy: 1, exemption: "added" }, null), "DEP_ALWAYS_ON");
  assert.equal(depCompat.crashHint({ policy: 3, exemption: "failed" }, null), "DEP_EXEMPTION_FAILED");
  assert.equal(depCompat.crashHint({ policy: 3, exemption: "added" }, null), null, "исключение действует");
  assert.equal(depCompat.crashHint({ policy: 2, exemption: "failed" }, null), null, "OptIn: hl.exe без NX_COMPAT и так работает");
  assert.equal(depCompat.crashHint({ policy: 1, exemption: "added" }, 0xc0000005), "DEP_ALWAYS_ON");
  assert.equal(depCompat.crashHint({ policy: 1, exemption: "added" }, -1073741819), "DEP_ALWAYS_ON", "знаковый код тот же");
  assert.equal(depCompat.crashHint({ policy: 1, exemption: "added" }, 1), null, "другой код — не DEP");
  assert.equal(depCompat.crashHint(null, null), null);
});

test("HL1 Windows: слой ставится перед запуском только на Windows", async () => {
  const calls = [];
  const fake = {
    POLICY: depCompat.POLICY,
    ensureExemption: (exe) => { calls.push(exe); return "added"; },
    queryPolicy: async () => 1,
    crashHint: depCompat.crashHint,
  };
  const win = new WindowsLaunchService({ delivery: {}, steamLocator: {}, depCompat: fake, platform: "win32" });
  win.prepareDep("C:\\hl.exe");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(calls, ["C:\\hl.exe"]);
  assert.equal(win.depCrashHint(null), "DEP_ALWAYS_ON");

  const linux = new WindowsLaunchService({ delivery: {}, steamLocator: {}, depCompat: fake, platform: "linux" });
  assert.equal(linux.prepareDep("C:\\hl.exe"), null);
  assert.equal(calls.length, 1);
});

test("HL1: настройки запуска собираются только в параметры, которые знает WON", () => {
  const goldsrc = require("../src/main/game-delivery/goldsrc-launch");
  const launchArgs = require("../src/main/game-delivery/launch-args");
  const argv = goldsrc.mergeArguments(config, {
    windowed: true, console: true, developer: true, condebug: true, nojoy: true, noaafonts: true,
    novid: true, dx9: true, high: true, borderless: true,
    width: 1280, height: 960, refreshRate: 144,
  });
  assert.deepEqual(argv.slice(config.arguments.length), [
    "-console", "-windowed", "-dev", "-condebug", "-nojoy", "-noaafonts", "-w", "1280", "-h", "960", "-freq", "144",
  ], "Source-флаги вроде -novid и -dx9 в HL1 не попадают");

  const override = goldsrc.mergeArguments(config, {
    width: 1280, height: 960, refreshRate: 144, launchArgs: "-w 800 -h 600 -freq 60",
  });
  assert.deepEqual(override.slice(config.arguments.length), ["-w", "800", "-h", "600", "-freq", "60"],
    "-w/-h и -freq игрока главнее настроек");

  const value = launchArgs.sanitizeSettings({ width: 1920, refreshRate: 7 }).settings;
  assert.equal(value.rinput, true, "RInput по умолчанию включён");
  assert.equal(value.width, 0, "разрешение без высоты отбрасывается");
  assert.equal(value.refreshRate, 0, "частота вне диапазона отбрасывается");
  assert.equal(launchArgs.sanitizeSettings({ rinput: false }).settings.rinput, false);
});

function rinputRig(t, settings, options = {}) {
  const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
  if (options.rinputFile !== false) writeFile(path.join(root, "RInput/RInput.exe"), "MZ");
  const launches = [];
  const killed = [];
  const warnings = [];
  const children = [];
  const runner = {
    launch: async (gameId, executable, argv, opts) => {
      const child = new EventEmitter();
      child.pid = 100 + launches.length;
      child.unref = () => {};
      launches.push({ gameId, executable, argv, opts });
      children.push(child);
      return { child, proton: { name: "test" }, prefix: "/pfx" };
    },
  };
  const service = new LinuxLaunchService({
    protonRunner: runner,
    steamLocator: { locateSteam: () => ({ installPath: "/steam" }), assertSteamRunning: () => ({ pid: 1 }) },
    killProcessGroup: (pid) => killed.push(pid),
    logger: { info() {}, warn: (m) => warnings.push(m), error() {} },
    bxtInjected: options.bxtInjected || (() => true),
    bxtPollMs: 1,
    bxtWaitMs: options.bxtWaitMs == null ? 1000 : options.bxtWaitMs,
    rinputMenuPatcher: options.rinputMenuPatcher || (() => { throw new Error("патч не включали"); }),
    rinputPatchWaitMs: options.rinputPatchWaitMs,
  });
  return { root, service, launches, killed, warnings, children, settings };
}

test("HL1 Linux: RInput ждёт hl.exe в том же префиксе и гасится вместе с игрой", async (t) => {
  const rig = rinputRig(t, null);
  await rig.service.startGoldSrc("hl1-won", { installPath: rig.root }, config, null);
  await rig.service.rinputStart;

  assert.equal(rig.launches.length, 2, "игра и RInput");
  const [game, rinput] = rig.launches;
  assert.equal(rinput.gameId, "hl1-won", "тот же префикс, что у игры");
  assert.equal(rinput.executable, path.join(rig.root, "RInput/RInput.exe"));
  assert.deepEqual(rinput.argv, ["hl.exe"]);
  assert.deepEqual(rinput.opts.steamBridge, game.opts.steamBridge, "тот же мост Proton, что у игры");
  assert.equal(rinput.opts.env.PROTON_HIDE_PROCESS_WINDOW, "1", "консоль RInput не отбирает фокус у игры");

  rig.children[0].emit("exit", 0);
  assert.deepEqual(rig.killed, [rig.children[1].pid], "ждущий RInput не переживает игру");

  const done = rinputRig(t, null);
  await done.service.startGoldSrc("hl1-won", { installPath: done.root }, config, null);
  await done.service.rinputStart;
  done.children[1].emit("exit", 0);
  done.children[0].emit("exit", 0);
  assert.deepEqual(done.killed, [], "завершившийся RInput не трогаем");
});

test("HL1 Linux: RInput ждёт, пока инжектор BXT внедрится в hl.exe", async (t) => {
  let injected = false;
  const rig = rinputRig(t, null, { bxtInjected: () => injected });
  await rig.service.startGoldSrc("hl1-won", { installPath: rig.root }, config, null);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(rig.launches.length, 1, "пока BXT внедряется, RInput не лезет в игру");
  injected = true;
  await rig.service.rinputStart;
  assert.equal(rig.launches.length, 2);

  const closed = rinputRig(t, null, { bxtInjected: () => false });
  await closed.service.startGoldSrc("hl1-won", { installPath: closed.root }, config, null);
  closed.children[0].emit("exit", 1);
  await closed.service.rinputStart;
  assert.equal(closed.launches.length, 1, "игра закрылась до BXT: RInput не нужен");

  const slow = rinputRig(t, null, { bxtInjected: () => false, bxtWaitMs: 10 });
  await slow.service.startGoldSrc("hl1-won", { installPath: slow.root }, config, null);
  await slow.service.rinputStart;
  assert.equal(slow.launches.length, 2, "без BXT RInput всё равно запускается");
  assert.ok(slow.warnings.some((m) => /Bunnymod XT не появился/.test(m)));
});

test("HL1 Linux: выключенный или отсутствующий RInput не мешает запуску игры", async (t) => {
  const off = rinputRig(t);
  await off.service.startGoldSrc("hl1-won", { installPath: off.root }, config, { rinput: false });
  assert.equal(off.launches.length, 1);

  const missing = rinputRig(t, null, { rinputFile: false });
  await missing.service.startGoldSrc("hl1-won", { installPath: missing.root }, config, null);
  assert.equal(missing.launches.length, 1);
  assert.ok(missing.warnings.some((m) => /RInput не найден/.test(m)));
});

test("HL1 Windows: RInput внедряется в найденный hl.exe без окна консоли", (t) => {
  const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
  writeFile(path.join(root, "RInput/RInput.exe"), "MZ");
  const spawned = [];
  const service = new WindowsLaunchService({
    delivery: {},
    steamLocator: {},
    spawnFn: (file, args, options) => {
      spawned.push({ file, args, options });
      const child = new EventEmitter();
      child.unref = () => {};
      return child;
    },
  });

  service.startRInput({ installPath: root }, config, null);
  assert.equal(spawned.length, 1);
  assert.equal(spawned[0].file, path.join(root, "RInput/RInput.exe"));
  assert.deepEqual(spawned[0].args, ["hl.exe"]);
  assert.equal(spawned[0].options.windowsHide, true);

  service.startRInput({ installPath: root }, config, { rinput: false });
  assert.equal(spawned.length, 1, "выключенный RInput не запускается");
});

test("HL1 Windows: курсор RInput в меню патчится через память hl.exe", async (t) => {
  const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
  writeFile(path.join(root, "RInput/RInput.exe"), "MZ");
  const statuses = ["waiting", "patched"];
  const calls = [];
  let closed = 0;
  const infos = [];
  const service = new WindowsLaunchService({
    delivery: {},
    steamLocator: {},
    logger: { info: (m) => infos.push(m), warn() {}, error() {} },
    inspector: { isAlive: () => true },
    rinputPatchPollMs: 1,
    rinputMenuSession: () => ({
      patch: async (name) => { calls.push(name); return statuses.shift(); },
      close: () => { closed++; },
    }),
    spawnFn: () => { const child = new EventEmitter(); child.unref = () => {}; return child; },
  });

  service.startRInput({ installPath: root }, config, { rinputMenuFix: true }, 4242);
  await service.rinputPatch;
  assert.deepEqual(calls, ["hl.exe", "hl.exe"], "ждём, пока RInput поставит перехваты");
  assert.equal(closed, 1, "помощник PowerShell закрывается после патча");
  assert.ok(infos.some((m) => /курсор RInput в меню исправлен/.test(m)));

  service.rinputPatch = null;
  service.startRInput({ installPath: root }, config, null, 4242);
  assert.equal(service.rinputPatch, null, "по умолчанию память игры не трогаем");
});

test("HL1 Windows: патч курсора RInput идёт через помощника с памятью процесса", async () => {
  const game = fakeGame();
  const { B } = game;
  const readMem = (address, length) => { const buf = Buffer.alloc(length); game.fakeFs.readSync(0, buf, 0, length, address); return buf; };
  const requests = [];
  const helper = {
    async request(line) {
      requests.push(line.split(" ")[0]);
      const [cmd, a, b] = line.split(" ");
      if (cmd === "pids") return { ok: true, pids: a === "hl" ? [4242] : [] };
      if (cmd === "module") return { ok: true, base: a === "rinput.dll" ? game.B : a === "user32.dll" ? game.U : null };
      if (cmd === "read") return { ok: true, hex: readMem(Number(a), Number(b)).toString("hex") };
      if (cmd === "write") { game.fakeFs.writeSync(0, Buffer.from(b, "hex"), 0, b.length / 2, Number(a)); return { ok: true }; }
      return { ok: true };
    },
    close() {},
  };
  const session = rinputMenuFixWin.openSession({ helper });
  assert.equal(await session.patch("hl.exe"), "patched");
  assert.equal(game.rinput.subarray(0x1530, 0x1532).toString("hex"), "eb18");
  assert.equal(game.writes[game.writes.length - 1], B + 0x1530, "переход на патч ставится последним");
  assert.equal(await session.patch("hl.exe"), "already");
  assert.ok(requests.includes("open") && requests.includes("close"));
  assert.ok(/Add-Type/.test(rinputMenuFixWin.HELPER_SCRIPT));
});

// Зависимости HL1 обновляются перед запуском, как update.bat сборки.
const dependencies = require("../src/main/game-delivery/goldsrc-dependencies");
const { Readable } = require("stream");

function fakeGitHub(releases, files) {
  const calls = { json: [], download: [] };
  return {
    calls,
    async getJson(url) {
      calls.json.push(url);
      const repo = url.replace("https://api.github.com/repos/", "").replace(/\/releases\/(latest|tags\/.+)$/, "");
      if (!releases[repo]) throw new Error(`нет релиза ${repo}`);
      return releases[repo];
    },
    async request(url) {
      calls.download.push(url);
      const response = Readable.from([files[url]]);
      response.statusCode = 200;
      return response;
    },
  };
}

function release(tag, name, url, body, digest) {
  const sha = require("crypto").createHash("sha256").update(body).digest("hex");
  return {
    tag_name: tag,
    assets: [
      { name: "3-BunnymodXT-CoFSteam-Windows.7z", size: 10, browser_download_url: "https://github.com/x/cof" },
      { name, size: body.length, browser_download_url: url, digest: digest || `sha256:${sha}` },
    ],
  };
}

// Обновление с GitHub проверяется на копии без закреплённой версии; закрепление — отдельными тестами.
const injectorOnly = { dependencies: config.dependencies.filter((d) => d.id === "injector").map((d) => Object.assign({}, d, { pin: undefined })) };

function withoutDigest(rel) {
  delete rel.assets[1].digest;
  return rel;
}

test("HL1: Injector закреплён: у его релиза нет SHA-256 от GitHub", () => {
  const injector = config.dependencies.find((d) => d.id === "injector");
  assert.equal(injector.pin.tag, "v3.2");
  assert.match(injector.pin.sha256, /^[a-f0-9]{64}$/);
});

test("HL1: файл без SHA-256 от GitHub не ставится", async (t) => {
  const root = fakeInstall(tempDir(t));
  writeFile(path.join(root, "Bunnymod XT/.hlsr-injector-version"), "v1");
  const body = Buffer.from("MZ new injector");
  const url = "https://github.com/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe";
  const client = fakeGitHub({ "YaLTeR/BunnymodXT-Injector": withoutDigest(release("v2", "Injector.exe", url, body)) }, { [url]: body });

  const [result] = await dependencies.updateDependencies(root, injectorOnly, { client });
  assert.equal(result.status, "failed");
  assert.equal(result.error.code, "CHECKSUM_MISMATCH");
  assert.equal(client.calls.download.length, 0);
  assert.equal(fs.readFileSync(path.join(root, "Bunnymod XT/Injector.exe"), "utf8"), "MZ");
});

test("HL1: закреплённая версия берётся по тегу и сверяется с хешем из лаунчера", async (t) => {
  const root = fakeInstall(tempDir(t));
  writeFile(path.join(root, "Bunnymod XT/.hlsr-injector-version"), "v1");
  const body = Buffer.from("MZ pinned injector");
  const url = "https://github.com/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe";
  const sha = require("crypto").createHash("sha256").update(body).digest("hex");
  const pinned = { dependencies: [Object.assign({}, injectorOnly.dependencies[0], { pin: { tag: "v2", sha256: sha } })] };
  const client = fakeGitHub({ "YaLTeR/BunnymodXT-Injector": withoutDigest(release("v2", "Injector.exe", url, body)) }, { [url]: body });

  const [result] = await dependencies.updateDependencies(root, pinned, { client });
  assert.deepEqual([result.status, result.to], ["updated", "v2"]);
  assert.deepEqual(client.calls.json, ["https://api.github.com/repos/YaLTeR/BunnymodXT-Injector/releases/tags/v2"]);
  assert.equal(fs.readFileSync(path.join(root, "Bunnymod XT/Injector.exe"), "utf8"), "MZ pinned injector");

  const quiet = fakeGitHub({}, {});
  const [again] = await dependencies.updateDependencies(root, pinned, { client: quiet });
  assert.equal(again.status, "up-to-date");
  assert.deepEqual(quiet.calls.json, [], "закреплённая версия на месте: в сеть не ходим");
});

test("HL1: закреплённая версия не принимает другой файл, даже если GitHub подтверждает его хеш", async (t) => {
  const root = fakeInstall(tempDir(t));
  writeFile(path.join(root, "Bunnymod XT/.hlsr-injector-version"), "v1");
  const body = Buffer.from("MZ tampered release");
  const url = "https://github.com/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe";
  const pinned = { dependencies: [Object.assign({}, injectorOnly.dependencies[0], { pin: { tag: "v2", sha256: "0".repeat(64) } })] };
  const client = fakeGitHub({ "YaLTeR/BunnymodXT-Injector": release("v2", "Injector.exe", url, body) }, { [url]: body });

  const [result] = await dependencies.updateDependencies(root, pinned, { client });
  assert.equal(result.status, "failed");
  assert.equal(result.error.code, "CHECKSUM_MISMATCH");
  assert.equal(fs.readFileSync(path.join(root, "Bunnymod XT/Injector.exe"), "utf8"), "MZ");
});

test("HL1: обновление зависимостей объявлено и не считается повреждением", () => {
  assert.deepEqual(config.dependencies.map((d) => d.repo), ["YaLTeR/BunnymodXT", "YaLTeR/BunnymodXT-Injector"]);
  const bxt = new RegExp(config.dependencies[0].asset, "i");
  assert.ok(bxt.test("1-BunnymodXT-Windows.7z"));
  assert.ok(!bxt.test("3-BunnymodXT-CoFSteam-Windows.7z"), "сборка для Cry of Fear не подходит");
  for (const dep of config.dependencies) {
    assert.ok(mutablePaths.matchesMutable(dep.target), dep.target);
    assert.ok(mutablePaths.matchesMutable(dep.versionFile), dep.versionFile);
  }
});

test("HL1: новая версия с GitHub ставится, старая версия записывается как было", async (t) => {
  const root = fakeInstall(tempDir(t));
  writeFile(path.join(root, "Bunnymod XT/.hlsr-injector-version"), "v1");
  const body = Buffer.from("MZ new injector");
  const url = "https://github.com/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe";
  const client = fakeGitHub({ "YaLTeR/BunnymodXT-Injector": release("v2", "Injector.exe", url, body) }, { [url]: body });

  const [result] = await dependencies.updateDependencies(root, injectorOnly, { client });
  assert.deepEqual([result.status, result.from, result.to], ["updated", "v1", "v2"]);
  assert.equal(fs.readFileSync(path.join(root, "Bunnymod XT/Injector.exe"), "utf8"), "MZ new injector");
  assert.equal(fs.readFileSync(path.join(root, "Bunnymod XT/.hlsr-injector-version"), "utf8"), "v2");
  assert.deepEqual(fs.readdirSync(path.join(root, "Bunnymod XT")).filter((f) => f.includes("update")), []);

  const [again] = await dependencies.updateDependencies(root, injectorOnly, { client });
  assert.equal(again.status, "up-to-date");
  assert.equal(client.calls.download.length, 1, "та же версия второй раз не качается");
});

test("HL1: битое обновление не трогает рабочий файл и не мешает запуску", async (t) => {
  const root = fakeInstall(tempDir(t));
  writeFile(path.join(root, "Bunnymod XT/.hlsr-injector-version"), "v1");
  const url = "https://github.com/YaLTeR/BunnymodXT-Injector/releases/download/v2/Injector.exe";
  const body = Buffer.from("MZ tampered");

  const cases = [
    ["CHECKSUM_MISMATCH", release("v2", "Injector.exe", url, body, `sha256:${"0".repeat(64)}`), body],
    ["EXTRACT_FAILED", release("v2", "Injector.exe", url, Buffer.from("<html>")), Buffer.from("<html>")],
    ["PROVIDER_LINK_FAILED", release("v2", "Injector.exe", "https://evil.example/Injector.exe", body), body],
  ];
  for (const [code, rel, payload] of cases) {
    const client = fakeGitHub({ "YaLTeR/BunnymodXT-Injector": rel }, { [rel.assets[1].browser_download_url]: payload });
    const [result] = await dependencies.updateDependencies(root, injectorOnly, { client });
    assert.equal(result.status, "failed", code);
    assert.equal(result.error.code, code);
    assert.equal(result.from, "v1");
    assert.equal(fs.readFileSync(path.join(root, "Bunnymod XT/Injector.exe"), "utf8"), "MZ", code);
  }

  const offline = { async getJson() { throw new Error("ECONNRESET"); } };
  const [result] = await dependencies.updateDependencies(root, injectorOnly, { client: offline });
  assert.equal(result.status, "failed");
});

test("HL1 Linux: зависимости обновляются до запуска игры, результат уходит в интерфейс", async (t) => {
  const root = fakeInstall(tempDir(t));
  const order = [];
  const service = new LinuxLaunchService({
    delivery: {},
    logger: { info() {}, warn() {}, error() {} },
    dependencyUpdater: async () => { order.push("update"); return [{ id: "bxt", status: "updated" }]; },
  });
  service.goldSrcLaunch = () => ({ executable: "x", argv: [], options: {}, files: { executable: "hl.exe" } });
  service.runner = { launch: async () => { order.push("launch"); return { child: new EventEmitter() }; } };
  service.startRInput = async () => {};
  const launched = await service.startGoldSrc("hl1-won", { installPath: root }, config, {});
  assert.deepEqual(order, ["update", "launch"]);
  assert.deepEqual(launched.dependencies, [{ id: "bxt", status: "updated" }]);
});

// Сборка HL1 качается с bxt.rs и ставится только с закреплённым SHA-256:
// другой архив в источнике замечается по Last-Modified и ETag, но не ставится.
const http = require("http");
const { execFileSync } = require("child_process");
const OriginUpdates = require("../src/main/game-delivery/origin-updates");
const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
const filesManifestLib = require("../src/main/game-delivery/files-manifest");

test("HL1: отпечаток закреплённой сборки bxt.rs соответствует встроенному архиву", () => {
  const base = OriginUpdates.ORIGINS["hl1-won"].baseline;
  assert.equal(base.size, manifests.MANIFESTS["hl1-won"].baseArchive.size, "отпечаток соответствует встроенному архиву");
  assert.ok(OriginUpdates.sameStamp(base, Object.assign({}, base, { lastModified: "другая" })), "ETag главнее даты");
  assert.ok(!OriginUpdates.sameStamp(base, Object.assign({}, base, { etag: '"x"' })));
});

function sevenZip() {
  // npm распаковывает 7za из 7zip-bin без бита исполнения; сборка выставляет
  // его в prepare-linux-7zip.js, а тесты идут раньше сборки.
  const bin = require("7zip-bin").path7za;
  if (process.platform !== "win32") fs.chmodSync(bin, fs.statSync(bin).mode | 0o111);
  return bin;
}

function packBuild(dir, archive, marker) {
  const root = path.join(dir, "Half-Life 2005 WON");
  fs.rmSync(root, { recursive: true, force: true });
  writeFile(path.join(root, "Half-Life/hl.exe"), "MZ");
  writeFile(path.join(root, "Half-Life/steam_appid.txt"), "70");
  writeFile(path.join(root, "Half-Life/valve_WON/liblist.gam"), 'game "Half-Life"\n');
  writeFile(path.join(root, "Bunnymod XT/Injector.exe"), "MZ");
  writeFile(path.join(root, "Bunnymod XT/BunnymodXT.dll"), "MZ");
  writeFile(path.join(root, "Half-Life/valve_WON/build.txt"), marker);
  fs.rmSync(archive, { force: true });
  execFileSync(sevenZip(), ["a", "-bd", "-y", archive, "Half-Life 2005 WON"], { cwd: dir, stdio: "ignore" });
  return root;
}

function serve(t, state) {
  const server = http.createServer((req, res) => {
    const body = fs.readFileSync(state.archive);
    res.writeHead(200, {
      "content-length": body.length,
      "last-modified": state.lastModified,
      etag: state.etag,
    });
    res.end(req.method === "HEAD" ? undefined : body);
  });
  t.after(() => server.close());
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}

test("HL1: другая сборка на bxt.rs без закреплённого SHA-256 не предлагается и не ставится", async (t) => {
  const dir = tempDir(t);
  const archive = path.join(dir, "build.7z");
  const buildRoot = packBuild(dir, archive, "v1");
  const v1Files = filesManifestLib.build({ root: buildRoot, gameId: "hl1-won", version: "1.0.0" }).manifest;
  const v1Size = fs.statSync(archive).size;
  const v1Sha = filesManifestLib.hashFileSync(archive);

  const state = { archive, lastModified: "Mon, 17 Jul 2023 11:23:57 GMT", etag: '"v1"' };
  const port = await serve(t, state);

  const base = manifests.MANIFESTS["hl1-won"];
  const manifest = Object.assign({}, base, {
    expectedInstalledSize: v1Files.totalSize,
    baseArchive: Object.assign({}, base.baseArchive, {
      size: v1Size,
      sha256: v1Sha,
      providers: [{ id: "bxt-rs", type: "local-http", status: "permanent", priority: 10, url: `http://127.0.0.1:${port}/won.7z` }],
    }),
  });
  const origins = { "hl1-won": { providerId: "bxt-rs", baseline: { version: "1.0.0", lastModified: state.lastModified, etag: '"v1"', size: v1Size } } };

  const warnings = [];
  const service = new GameDeliveryService({
    libraryPath: path.join(dir, "library"),
    launcherVersion: "0.3.16",
    env: { GAME_DELIVERY_ALLOW_LOCAL_HTTP: "1" },
    manifests: { "hl1-won": manifest },
    filesManifests: { "hl1-won": { "1.0.0": v1Files } },
    channelService: null,
    depotService: { assertOwns: async () => true },
    updateCheckIntervalMs: 0,
    logger: { info() {}, warn: (line) => warnings.push(line), error() {} },
  });
  service.originUpdates.origins = origins;
  t.after(() => service.dispose());

  assert.equal((await service.install("hl1-won")).status, "installed");
  const game = service.installedOf("hl1-won").installPath;
  const build = path.join(game, "Half-Life/valve_WON/build.txt");

  let check = await service.checkForUpdates("hl1-won");
  assert.equal(check.update.updateAvailable, false, "та же сборка — обновлять нечего");

  // На bxt.rs лежит другая сборка: её SHA-256 нигде не закреплён.
  packBuild(dir, archive, "v2");
  Object.assign(state, { lastModified: "Sun, 20 Sep 2026 00:05:09 GMT", etag: '"v2"' });

  check = await service.checkForUpdates("hl1-won");
  assert.equal(check.update.updateAvailable, false, "незакреплённая сборка не предлагается");
  assert.equal((await service.applyUpdate("hl1-won")).status, "up-to-date");
  assert.equal(service.installedOf("hl1-won").version, "1.0.0");
  assert.equal(fs.readFileSync(build, "utf8"), "v1");
  assert.equal(warnings.filter((line) => line.includes("другая сборка")).length, 1, warnings.join("\n"));

  // Даже если цель без хэша как-то дошла до установки, полный архив не ставится.
  service.originUpdates.target = async () => ({
    source: "origin",
    version: "1.20260920.509",
    patches: [],
    fullArchive: Object.assign({}, manifest.baseArchive, { version: "1.20260920.509", sha256: null }),
  });
  check = await service.checkForUpdates("hl1-won");
  assert.equal(check.update.updateAvailable, true);
  await assert.rejects(service.applyUpdate("hl1-won"), { code: "CHECKSUM_MISMATCH" });
  assert.equal(service.installedOf("hl1-won").version, "1.0.0");
  assert.equal(fs.readFileSync(build, "utf8"), "v1");
});

// --- паки HUD и прицелов ---

const goldsrcPacks = require("../src/main/game-delivery/goldsrc-packs");
const crypto = require("crypto");

function sha(text) {
  return crypto.createHash("sha256").update(text).digest("hex");
}

// Вместо Google Drive и 7-Zip: «архив» — это строка с именем папки-образца,
// а распаковка копирует эту папку.
function packFixtures(dir) {
  const fixtures = path.join(dir, "fixtures");
  writeFile(path.join(fixtures, "hud/big AG HUD/hud.txt"), "AG HUD");
  writeFile(path.join(fixtures, "hud/big AG HUD/hud_numbers_0.spr"), "AG0");
  writeFile(path.join(fixtures, "cross/crosshair by execut4ble/xhair.spr"), "XHAIR");
  writeFile(
    path.join(fixtures, "cross/crosshair by execut4ble/weapon_tripmine.txt"),
    "8\r\nweapon\t\t320 320hud1\t160\t40\t80\t20\r\ncrosshair\t\t320 xhair\t0\t128\t128\t128\r\n" +
      "weapon\t\t640 640hud3\t0\t45\t170\t45\r\ncrosshair\t\t640 xhair\t0\t128\t128\t128\r\n"
  );
  writeFile(path.join(fixtures, "changer/crosshairs/1px/red.spr"), "RED-1PX");
  const entry = (id, root, kind) => ({
    kind,
    title: id,
    author: "test",
    driveId: id,
    size: Buffer.byteLength(`ZIP:${id}`),
    sha256: sha(`ZIP:${id}`),
    root,
  });
  const catalog = {
    "ag-hud": entry("hud", "big AG HUD", "hud"),
    "execut4ble-crosshair": entry("cross", "crosshair by execut4ble", "crosshair"),
    "crosshair-changer": entry("changer", "crosshairs", "crosshair"),
  };
  const downloads = [];
  const client = {
    async request(url) {
      const id = new URL(url).searchParams.get("id");
      downloads.push(id);
      const response = new EventEmitter();
      response.statusCode = 200;
      response.body = Buffer.from(`ZIP:${id}`);
      response.resume = () => {};
      return response;
    },
    async readBody(response) {
      return response.body;
    },
  };
  const archive = {
    extract(file, destination) {
      const id = fs.readFileSync(file, "utf8").slice(4);
      fs.cpSync(path.join(fixtures, id), destination, { recursive: true });
      return { promise: Promise.resolve() };
    },
  };
  return { catalog, client, archive, downloads };
}

function packInstall(root) {
  fakeInstall(root, "70");
  const sprites = path.join(root, "Half-Life/valve_WON/sprites");
  writeFile(path.join(sprites, "weapon_tripmine.txt"), "7\r\nweapon\t\t320 320hud1\t160\t80\t80\t20\r\ncrosshair\t\t640 crosshairs\t48\t48\t24\t24");
  writeFile(path.join(root, "Half-Life/valve/sprites/crosshairs.spr"), "VALVE-CROSS");
  writeFile(path.join(root, "Half-Life/valve/sprites/hud.txt"), "VALVE HUD");
  return sprites;
}

test("HL1: паки объявлены с автором, закреплённым хэшем и не считаются повреждением", () => {
  assert.ok(config.packs, "у HL1 есть папка спрайтов для паков");
  for (const id of Object.keys(goldsrcPacks.PACKS)) {
    const pack = goldsrcPacks.PACKS[id];
    assert.match(pack.sha256, /^[a-f0-9]{64}$/, id);
    assert.ok(pack.author && pack.size > 0, id);
  }
  assert.ok(mutablePaths.matchesMutable("Half-Life/valve_WON/sprites/hud.txt"));
  assert.ok(mutablePaths.matchesMutable("Half-Life/valve_WON/sprites/sub/x.spr"));
  assert.ok(!mutablePaths.matchesMutable("Half-Life/valve/sprites/hud.txt"), "спрайты valve проверяются как раньше");

  assert.deepEqual(goldsrcPacks.sanitizePacks({ hud: "evil", crosshair: "crosshair-changer", changerShape: "../x" }), {
    hud: "default",
    crosshair: "crosshair-changer",
    changerShape: "Default",
    changerColor: "green",
  });
});

test("HL1: прицел execut4ble меняет только строки прицела, иконки оружия остаются от WON", () => {
  const base = Buffer.from("7\r\nweapon\t\t320 320hud1\t160\t80\t80\t20\r\nammo\t\t320 320hud2\t72\t34\t18\t18\r\ncrosshair\t\t640 crosshairs\t48\t48\t24\t24");
  const pack = Buffer.from("8\r\nweapon\t\t320 320hud1\t160\t40\t80\t20\r\ncrosshair\t\t320 xhair\t0\t128\t128\t128\r\nzoom\t\t640 xhair\t0\t128\t128\t128\r\n");
  const lines = goldsrcPacks.mergeCrosshairLines(base, pack).toString("latin1").trim().split("\r\n");
  assert.deepEqual(lines, [
    "4",
    "weapon\t\t320 320hud1\t160\t80\t80\t20",
    "ammo\t\t320 320hud2\t72\t34\t18\t18",
    "crosshair\t\t320 xhair\t0\t128\t128\t128",
    "zoom\t\t640 xhair\t0\t128\t128\t128",
  ]);
});

test("HL1: выбранные паки ставятся, а при отключении всё возвращается как было", async (t) => {
  const dir = tempDir(t);
  const root = path.join(dir, "game");
  const sprites = packInstall(root);
  const tripmineBefore = fs.readFileSync(path.join(sprites, "weapon_tripmine.txt"));
  const fx = packFixtures(dir);
  const opts = { catalog: fx.catalog, client: fx.client, archive: fx.archive };

  const applied = await goldsrcPacks.applyPacks(root, config, { packs: { hud: "ag-hud", crosshair: "execut4ble-crosshair" } }, opts);
  assert.equal(applied.status, "applied");
  assert.equal(fs.readFileSync(path.join(sprites, "hud.txt"), "utf8"), "AG HUD");
  assert.equal(fs.readFileSync(path.join(sprites, "xhair.spr"), "utf8"), "XHAIR");
  const tripmine = fs.readFileSync(path.join(sprites, "weapon_tripmine.txt"), "latin1");
  assert.match(tripmine, /320hud1\t160\t80/, "иконка мины из WON");
  assert.match(tripmine, /crosshair\t\t640 xhair/, "прицел из пака");
  assert.doesNotMatch(tripmine, /crosshairs\t48/, "старый прицел убран");

  // Смена прицела: файлы прошлого пака уходят, оригинал мины возвращается.
  await goldsrcPacks.applyPacks(root, config, {
    packs: { hud: "ag-hud", crosshair: "crosshair-changer", changerShape: "1px", changerColor: "red" },
  }, opts);
  assert.equal(fs.readFileSync(path.join(sprites, "crosshairs.spr"), "utf8"), "RED-1PX");
  assert.ok(!fs.existsSync(path.join(sprites, "xhair.spr")));
  assert.deepEqual(fs.readFileSync(path.join(sprites, "weapon_tripmine.txt")), tripmineBefore);

  const off = await goldsrcPacks.applyPacks(root, config, { packs: {} }, opts);
  assert.equal(off.status, "default");
  assert.deepEqual(fs.readdirSync(sprites).sort(), ["weapon_tripmine.txt"]);
  assert.deepEqual(fs.readFileSync(path.join(sprites, "weapon_tripmine.txt")), tripmineBefore);

  // Скачанные архивы лежат в кэше: повторный выбор не качает заново.
  const before = fx.downloads.length;
  await goldsrcPacks.applyPacks(root, config, { packs: { hud: "ag-hud" } }, opts);
  assert.equal(fx.downloads.length, before);
});

test("HL1: подменённый на Drive пак не ставится и не трогает прошлые файлы", async (t) => {
  const dir = tempDir(t);
  const root = path.join(dir, "game");
  const sprites = packInstall(root);
  const fx = packFixtures(dir);
  const opts = { catalog: fx.catalog, client: fx.client, archive: fx.archive };
  await goldsrcPacks.applyPacks(root, config, { packs: { hud: "ag-hud" } }, opts);

  const tampered = Object.assign({}, fx.catalog, {
    "crosshair-changer": Object.assign({}, fx.catalog["crosshair-changer"], { sha256: sha("другой файл") }),
  });
  const result = await goldsrcPacks.applyPacks(root, config, {
    packs: { hud: "ag-hud", crosshair: "crosshair-changer" },
  }, Object.assign({}, opts, { catalog: tampered }));
  assert.equal(result.status, "failed");
  assert.equal(result.error.code, "CHECKSUM_MISMATCH");
  assert.equal(fs.readFileSync(path.join(sprites, "hud.txt"), "utf8"), "AG HUD", "прошлый HUD остался");
  assert.ok(!fs.existsSync(path.join(sprites, "crosshairs.spr")));
});

test("HL1: файл, который после пака поменяла сборка, остаётся новым", async (t) => {
  const dir = tempDir(t);
  const root = path.join(dir, "game");
  const sprites = packInstall(root);
  const fx = packFixtures(dir);
  const opts = { catalog: fx.catalog, client: fx.client, archive: fx.archive };
  await goldsrcPacks.applyPacks(root, config, { packs: { crosshair: "execut4ble-crosshair" } }, opts);

  writeFile(path.join(sprites, "weapon_tripmine.txt"), "NEW BUILD");
  await goldsrcPacks.applyPacks(root, config, { packs: {} }, opts);
  assert.equal(fs.readFileSync(path.join(sprites, "weapon_tripmine.txt"), "utf8"), "NEW BUILD");
});

test("HL1 Linux: паки применяются до запуска, результат уходит в интерфейс", async (t) => {
  const calls = [];
  const service = Object.create(LinuxLaunchService.prototype);
  service.logger = { info() {}, warn() {}, error() {} };
  service.dependencyUpdater = async () => { calls.push("deps"); return []; };
  service.packApplier = async (installPath, cfg, settings) => {
    calls.push(`packs:${settings.packs.hud}`);
    return { status: "applied", packs: ["ag-hud"] };
  };
  service.goldSrcLaunch = () => { calls.push("plan"); return { files: { executable: "hl.exe" }, executable: "i", argv: [], options: {} }; };
  service.runner = { launch: async () => { calls.push("launch"); return { child: new EventEmitter() }; } };
  service.startRInput = async () => {};
  const result = await service.startGoldSrc("hl1-won", { installPath: "/x" }, config, { packs: { hud: "ag-hud" } });
  assert.deepEqual(calls, ["deps", "packs:ag-hud", "plan", "launch"]);
  assert.equal(result.packs.status, "applied");
});

// --- кастомизация через Bunnymod XT ---

const goldsrcPersonalization = require("../src/main/game-delivery/goldsrc-personalization");
const goldsrcSettings = require("../src/shared/goldsrc-settings");
const launchArgs = require("../src/main/game-delivery/launch-args");

// Разрешённые правилами HL1 на speedrun.com команды, которые пишет лаунчер.
const HL1_WHITELIST = new Set([
  "bxt_hud_game_color", "bxt_hud_game_alpha", "bxt_hud_color", "crosshair", "bxt_cross",
  "bxt_cross_color", "bxt_cross_alpha", "bxt_cross_thickness", "bxt_cross_size", "bxt_cross_gap",
  "bxt_cross_outline", "bxt_cross_circle_radius", "bxt_cross_dot_color", "bxt_cross_dot_size",
  "bxt_cross_top_line", "bxt_cross_bottom_line", "bxt_cross_left_line", "bxt_cross_right_line",
  "bxt_hud_timer", "bxt_hud_speedometer", "bxt_hud_jumpspeed", "bxt_hud_viewangles",
  "default_fov", "bxt_fix_widescreen_fov", "bxt_viewmodel_fov", "bxt_viewmodel_semitransparent",
  // В whitelist GoldSrc без обязательного значения, то есть любые значения.
  "bxt_hud_timer_anchor", "bxt_hud_speedometer_anchor", "bxt_hud_jumpspeed_anchor", "bxt_hud_viewangles_anchor",
  "bxt_hud_jumpdistance", "bxt_hud_precision", "bxt_hud_incorrect_fps_indicator", "bxt_remove_viewmodel",
  "bxt_viewmodel_ofs_forward", "bxt_viewmodel_ofs_right", "bxt_viewmodel_ofs_up", "bxt_viewmodel_bob_angled",
  "bxt_viewmodel_disable_idle", "bxt_viewmodel_disable_equip",
]);

function cfgValues(text) {
  const values = {};
  for (const line of text.split("\r\n")) {
    const match = /^(\S+) "([^"]*)"$/.exec(line);
    if (match) values[match[1]] = match[2];
  }
  return values;
}

test("HL1: cfg BXT пишет только разрешённые команды, по умолчанию — как в игре", () => {
  const values = cfgValues(goldsrcPersonalization.toCfgLines({}));
  for (const name of Object.keys(values)) assert.ok(HL1_WHITELIST.has(name), `${name} нет в белом списке`);
  assert.equal(Object.keys(values).length, HL1_WHITELIST.size);
  // Умолчания BXT из cvars.hpp: пустой цвет и прозрачность 0 — цвет игры.
  assert.equal(values.bxt_hud_game_color, "");
  assert.equal(values.bxt_hud_game_alpha, "0");
  assert.equal(values.crosshair, "1");
  assert.equal(values.bxt_cross, "0");
  assert.equal(values.bxt_hud_speedometer, "1");
  assert.equal(values.default_fov, "90");
  // Положение панелей по умолчанию — как в BXT (cvars.hpp).
  assert.equal(values.bxt_hud_speedometer_anchor, "0.5 1");
  assert.equal(values.bxt_hud_timer_anchor, "0 0.5");
  assert.equal(values.bxt_hud_viewangles_anchor, "1 0");
  assert.equal(values.bxt_hud_precision, "6");
  assert.equal(values.bxt_hud_incorrect_fps_indicator, "1");
});

test("HL1: положение спидометра и сдвиг оружия переводятся в команды BXT", () => {
  const values = cfgValues(goldsrcPersonalization.toCfgLines({
    bxt: { speedometerX: 25, speedometerY: 60, viewmodelUp: -3, removeViewmodel: true, precision: 99 },
  }));
  assert.equal(values.bxt_hud_speedometer_anchor, "0.25 0.6");
  assert.equal(values.bxt_viewmodel_ofs_up, "-3");
  assert.equal(values.bxt_remove_viewmodel, "1");
  // Значения за пределами поля обрезаются: BXT больше 16 знаков не показывает.
  assert.equal(values.bxt_hud_precision, "16");
  // Положение — только у включённой панели: у выключенной поле скрыто.
  const field = goldsrcSettings.FIELDS.find((item) => item.key === "jumpspeedX");
  assert.equal(goldsrcSettings.available(field, { jumpspeed: false }), false);
  assert.equal(goldsrcSettings.available(field, { jumpspeed: true }), true);
});

test("HL1: цвет HUD, прицел BXT и панели переводятся в команды BXT", () => {
  const values = cfgValues(goldsrcPersonalization.toCfgLines({
    personalization: {
      hudEnabled: true, hudColor: { r: 0, g: 200, b: 255 }, hudAlpha: 200, hudAffectsCrosshair: true,
      crossEnabled: true, crossColor: { r: 255, g: 0, b: 0 }, crossOutline: true, crossOutlineThickness: 2,
      crossHideDefault: true,
    },
    bxt: { timer: true, speedometer: false, fov: 300, viewmodelFov: 90 },
  }));
  // Прозрачность 200 внесена в цвет: со своим цветом BXT её не учитывает.
  assert.equal(values.bxt_hud_game_color, "0 157 200");
  assert.equal(values.bxt_hud_color, "0 157 200");
  assert.equal(values.bxt_hud_game_alpha, "0");
  assert.equal(values.bxt_cross_color, "0 200 255", "цвет прицела как у HUD");
  assert.equal(values.bxt_cross_outline, "2", "обводка в BXT — толщина линии");
  assert.equal(values.crosshair, "0");
  assert.equal(values.bxt_hud_timer, "1");
  assert.equal(values.bxt_hud_speedometer, "0");
  assert.equal(values.default_fov, "120", "поле зрения ограничено");
  assert.equal(values.bxt_viewmodel_fov, "90");
});

const DEFAULT_USERCONFIG = require("../src/main/game-delivery/hl1-default-userconfig");

// Стандартный userconfig.cfg лаунчера 0.4.0: комментарии были на русском.
const RU_040_USERCONFIG = [
  "exec WON.cfg // Не удалять: fps_max 100 и clockwindow 0 как в оригинальном движке, без них ран не засчитают",
  "",
  "// Обязательно по правилам speedrun.com",
  "_bxt_save_runtime_data_in_demos 1 // Данные BXT в демках, по ним модераторы проверяют ран",
  "cl_showfps 1 // Счётчик FPS в углу экрана, обязательно включён",
  "",
  "// Старт рана: сложность, первая карта, таймер BXT и запись демок run_1, run_2 и дальше",
  "// Сложность во время рана менять нельзя, поэтому она задаётся здесь, до старта",
  `alias run "${require("../src/shared/bind-presets").HL1_RUN_ALIAS}"`,
  'bind p "run" // Начать ран заново',
  "",
  "// После финиша запись демок останавливает команда stop в консоли",
  "// Демки сдаются вместе с раном, храни демки своего рекорда",
  "",
  "bxt_hud_incorrect_fps_indicator 0",
  "",
  "// Свои бинды и alias пиши ниже. Настройки игры удобнее менять в лаунчере:",
  "// Настройки игры, вкладка «Конфиги»",
  "",
].join("\r\n");

test("HL1: стандартный userconfig.cfg прошлых версий лаунчера считается заменяемым", () => {
  const playerConfig = require("../src/main/game-delivery/player-config");
  // Файл читается как latin1: русские комментарии - байты UTF-8.
  const asFile = (text) => Buffer.from(text, "utf8").toString("latin1");
  const previous = RU_040_USERCONFIG.split("\r\n").filter((line) => !/^cl_showfps /.test(line)).join("\r\n");
  assert.equal(playerConfig.isStock("hl1-won", asFile(`${previous}exec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n`)), true);
  assert.equal(playerConfig.isStock("hl1-won", asFile(`${RU_040_USERCONFIG}\r\nexec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n`)), true);
  assert.equal(playerConfig.isStock("hl1-won", asFile(`${RU_040_USERCONFIG}\r\nbind f "+duck"\r\n`)), false, "свои строки игрока - не стоковый");
});
test("HL1: стандартный userconfig.cfg — по правилам WON и без ; в комментариях", () => {
  const lines = DEFAULT_USERCONFIG.TEXT.split("\r\n");
  assert.ok(/^exec WON\.cfg\b/.test(lines[0]), "WON.cfg первой строкой");
  assert.ok(lines.some((line) => /^_bxt_save_runtime_data_in_demos 1\b/.test(line)), "данные BXT в демках");
  assert.ok(/^[\t\r\n\x20-\x7e]*$/.test(DEFAULT_USERCONFIG.TEXT), "только ASCII, без русских комментариев");
  assert.ok(lines.some((line) => /^cl_showfps 1\b/.test(line)), "счётчик FPS включён");
  assert.ok(lines.includes(`alias run "${bindPresets.HL1_RUN_ALIAS}"`));
  for (const line of lines) {
    const comment = line.indexOf("//");
    if (comment >= 0) assert.ok(!line.slice(comment).includes(";"), line);
  }
});

test("HL1: паки и BXT в окне — один список, сохраняются раздельно", () => {
  const flat = goldsrcSettings.toFlat(
    { hud: "ag-hud", crosshair: "crosshair-changer", changerShape: "1px", changerColor: "red" },
    { timer: true, fov: 100 }
  );
  const back = goldsrcSettings.fromFlat(flat);
  assert.deepEqual(back.packs, { hud: "ag-hud", crosshair: "crosshair-changer", changerShape: "1px", changerColor: "red" });
  assert.equal(back.bxt.timer, true);
  assert.equal(back.bxt.fov, 100);

  const shape = goldsrcSettings.FIELDS.find((field) => field.key === "changerShape");
  assert.equal(goldsrcSettings.available(shape, { packCrosshair: "crosshair-changer" }), true);
  assert.equal(goldsrcSettings.available(shape, { packCrosshair: "execut4ble-crosshair" }), false);

  const saved = launchArgs.sanitizeSettings({ packs: back.packs, bxt: Object.assign({ bogus: 1 }, back.bxt) }).settings;
  assert.deepEqual(saved.packs, back.packs);
  assert.deepEqual(saved.bxt, back.bxt);
});

const goldsrcBinds = require("../src/shared/game-binds");
const ghostingPersonalizationModule = require("../src/main/game-delivery/ghosting-personalization");
const bindPresets = require("../src/shared/bind-presets");
const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const cfgImport = require("../src/shared/cfg-import");

test("HL1 бинды: cfg разбирается как в движке, цепочка exec и unbindall учитываются", () => {
  const files = {
    "config.cfg": [
      'unbindall',
      'bind "w" "+forward"',
      'bind "SPACE" "+jump" // комментарий',
      'bind "q" "lastinv"; bind "E" "+use"',
      'alias "jb" "+jump;wait;-jump"',
      'exec userconfig.cfg',
    ].join("\r\n"),
    "userconfig.cfg": 'bind "mwheeldown" "jb"\nunbind "q"\nexec scripts.cfg\nexec hlsr-binds.cfg\nexec config.cfg',
    "scripts.cfg": 'bind "f" "bxt_append; +duck"',
    "hlsr-binds.cfg": 'bind "w" "+back"',
  };
  const result = goldsrcBinds.readBinds((name) => (name in files ? files[name] : null));
  assert.equal(result.binds.w.command, "+forward", "свой hlsr-binds.cfg лаунчер не читает");
  assert.equal(result.binds.SPACE.command, "+jump");
  assert.equal(result.binds.e.command, "+use", "имя из одной буквы приводится к нижнему регистру");
  assert.equal(result.binds.q, undefined, "unbind снимает бинд");
  assert.equal(result.binds.MWHEELDOWN.source, "userconfig.cfg");
  assert.equal(result.binds.f.command, "bxt_append; +duck");
  assert.equal(result.aliases.jb, "+jump;wait;-jump");
  assert.deepEqual(result.files, ["config.cfg", "userconfig.cfg", "scripts.cfg"], "повторный exec не зацикливает");
});

test("HL1 бинды: импорт cfg переносит бинды, unbindall снимает лишнее, alias из файла пропускается", () => {
  const fileBinds = {
    w: { command: "+forward", source: "config.cfg" },
    e: { command: "+use", source: "config.cfg" },
    SPACE: { command: "+jump", source: "config.cfg" },
  };
  const text = [
    "unbindall",
    'alias +jb "+jump"',
    'bind "w" "+forward"',
    'bind "SPACE" "+jb"',
    'bind "f" "impulse 100" // фонарик',
    'bind "MWHEELDOWN" "+jump"',
    'bind "NOSUCHKEY" "x"',
  ].join("\n");
  const result = goldsrcBinds.importOverrides(text, { fileBinds, overrides: {}, aliases: {} });
  assert.deepEqual(result.overrides, {
    f: { command: "impulse 100", base: "" },
    MWHEELDOWN: { command: "+jump", base: "" },
    e: { command: "", base: "+use" },
  });
  assert.equal(result.changed, 2);
  assert.equal(result.unbound, 1);
  assert.equal(result.unchanged, 1);
  assert.equal(result.skippedAlias, 1);
  assert.equal(result.skippedKey, 1);

  // alias, который уже есть в игре, или команда заготовки лаунчера — не пропуск.
  const known = goldsrcBinds.importOverrides(text, { fileBinds, overrides: {}, aliases: { "+JB": "+jump" } });
  assert.deepEqual(known.overrides.SPACE, { command: "+jb", base: "+jump" });

  // Повторный импорт того же файла ничего не меняет.
  const again = goldsrcBinds.importOverrides(text, { fileBinds, overrides: result.overrides, aliases: {} });
  assert.deepEqual(again.overrides, result.overrides);
  assert.equal(again.changed + again.unbound, 0);
});

test("HL1 бинды: переназначения пишутся в hlsr-binds.cfg, возврат чистится, когда игра его уже хранит", () => {
  const overrides = goldsrcBinds.sanitizeOverrides({
    W: { command: "+back", base: "+forward" },
    q: { command: "", base: "lastinv" },
    'bad"key': { command: "+jump", base: "" },
    e: { command: 'say "hi"', base: "+use" },
  });
  assert.deepEqual(Object.keys(overrides).sort(), ["q", "w"], "кавычки в клавише и команде отбрасываются");
  const cfg = goldsrcBinds.toCfgLines(overrides);
  assert.match(cfg, /bind "w" "\+back"/);
  assert.match(cfg, /unbind "q"/);

  const reset = goldsrcBinds.resetOverrides(overrides);
  assert.deepEqual(reset.w, { command: "+forward", base: "+forward" });
  // Игра после выхода записала прежний бинд в config.cfg: возврат больше не нужен.
  const pruned = goldsrcBinds.pruneOverrides(reset, { w: { command: "+forward" }, q: { command: "+use" } });
  assert.deepEqual(Object.keys(pruned), ["q"]);
});

test("HL1 бинды: переназначение пишется строкой в файл игрока, hlsr-binds.cfg не создаётся", (t) => {
  const root = tempDir(t);
  const gameDir = path.join(root, "Half-Life/valve_WON");
  fs.mkdirSync(gameDir, { recursive: true });
  fs.writeFileSync(path.join(gameDir, "userconfig.cfg"), "fps_max 100\r\n");
  const config = manifests.LAUNCH_CONFIG["hl1-won"];
  const binds = { MOUSE4: { command: "+duck", base: "" } };
  for (let i = 0; i < 2; i += 1) gameBindsInstall.applyBinds(root, "hl1-won", config.gameDirectory, binds, { language: "english" });
  const userconfig = fs.readFileSync(path.join(gameDir, "userconfig.cfg"), "latin1");
  assert.equal(userconfig.split('bind "mouse4" "+duck"').length - 1, 1, "строка одна");
  assert.ok(userconfig.startsWith("fps_max 100\r\n"), "своё не тронуто");
  assert.doesNotMatch(userconfig, /exec hlsr-binds/);
  assert.equal(fs.existsSync(path.join(gameDir, "hlsr-binds.cfg")), false);
});

test("Бинды: скрипт из заготовки HL1 пишет свои alias до биндов, один раз", (t) => {
  const root = tempDir(t);
  const config = manifests.LAUNCH_CONFIG["hl1-won"];
  const first = { f: { command: "+fps_20_hold", base: "" }, g: { command: "+tau", base: "" }, h: { command: "tauswitch", base: "" } };
  const apply = (binds, previous) => gameBindsInstall.applyBinds(root, "hl1-won", config.gameDirectory, binds, { language: "english", previous });
  apply(first);
  const cfg = fs.readFileSync(path.join(root, "Half-Life/valve_WON/userconfig.cfg"), "latin1");
  const lines = cfg.split("\r\n");
  assert.ok(lines.includes('alias +fps_20_hold "fps_max 20; alias -fps_20_hold fps_20_reset"'));
  assert.ok(lines.includes("alias _taubo"), "пустой alias пишется без кавычек, как в гайде");
  assert.equal(lines.filter((line) => line.startsWith("alias +tau ")).length, 1, "общие alias не повторяются");
  assert.ok(lines.includes("// HLSR script +fps_20_hold begin"), "alias лежит в блоке со своим именем");

  apply({ f: { command: "", base: "+fps_20_hold" } }, first);
  assert.doesNotMatch(fs.readFileSync(path.join(root, "Half-Life/valve_WON/userconfig.cfg"), "latin1"), /alias|HLSR script/, "снятый скрипт уходит из файла игрока");
});

test("Бинды: в HL1 скрипты отмечены для Scripted, обычные команды — нет", () => {
  const hl1 = bindPresets.catalogFor("hl1-won");
  assert.equal(hl1.runModes, true);
  assert.equal(bindPresets.isScripted(hl1, "+fps_20_hold"), true);
  assert.equal(bindPresets.isScripted(hl1, "+bxt_tas_ducktap"), true);
  assert.equal(bindPresets.isScripted(hl1, "fps_max 20"), false);
  assert.equal(bindPresets.isScripted(hl1, "+jump"), false);
  assert.equal(bindPresets.isScripted(hl1, "+jump; wait; -jump"), true, "своя команда из нескольких — скрипт");
  assert.equal(bindPresets.presetOf(hl1, "SAVE X;load x").label.en, "Save & load", "пробелы и регистр не важны");
  for (const id of Object.keys(bindPresets.CATALOGS)) {
    const catalog = bindPresets.catalogFor(id);
    const categories = new Set(catalog.categories.map((item) => item.id));
    for (const preset of catalog.presets) {
      assert.ok(categories.has(preset.category), `${id}: ${preset.command}`);
      assert.equal(goldsrcBinds.commandError(preset.command), null, `${id}: ${preset.command}`);
      for (const [, value] of preset.aliases) assert.ok(!value.includes('"'), `${id}: ${preset.command}`);
    }
  }
  assert.equal(bindPresets.catalogFor("ghosting-renewed").runModes, false);
  const steampipe = process.platform === "linux" ? "save /;reload" : "save :;reload";
  assert.equal(bindPresets.STEAMPIPE_SAVE_DELETION, steampipe, "под Linux удаление сейва через «save /»");
  assert.ok(bindPresets.presetOf(bindPresets.catalogFor("hl2-5288"), steampipe));
  assert.ok(bindPresets.presetOf(bindPresets.catalogFor("hl2-4260"), bindPresets.PRE_STEAMPIPE_SAVE_DELETION));
});

const hl1Guide = require("../src/shared/hl1-guide");

test("Скрипты гайда: каждое действие — заготовка HL1, alias скриптов не конфликтуют", () => {
  const hl1 = bindPresets.catalogFor("hl1-won");
  assert.ok(hl1Guide.SCRIPTS.length >= 70, "в гайде больше 70 скриптов");
  const values = new Map();
  for (const preset of hl1.presets) {
    for (const [name, value] of preset.aliases) {
      if (values.has(name)) assert.equal(values.get(name), value, `alias ${name} в двух заготовках с разным кодом`);
      values.set(name, value);
    }
  }
  for (const script of hl1Guide.SCRIPTS) {
    assert.ok(script.actions.length, `${script.id}: нет действий`);
    assert.ok(hl1Guide.CHAPTERS.some((chapter) => chapter.id === script.chapter), script.id);
    for (const action of script.actions) {
      const preset = bindPresets.presetOf(hl1, action.command);
      assert.ok(preset, `${script.id}: ${action.command}`);
      assert.equal(goldsrcBinds.commandError(action.command), null, `${script.id}: ${action.command}`);
      if (action.guideKey) assert.ok(goldsrcBinds.normalizeKey(action.guideKey), `${script.id}: клавиша ${action.guideKey}`);
    }
    // Своё действие скрипта приносит все его alias. Общие команды вроде
    // +bxt_tas_jumpbug остаются прежними заготовками без alias.
    const own = script.actions.filter((action) => {
      const preset = bindPresets.presetOf(hl1, action.command);
      return preset.guide === script.id || preset.aliases.length;
    });
    assert.ok(own.length, `${script.id}: ни одно действие не приносит alias`);
    for (const action of own) {
      const written = new Map(bindPresets.aliasesFor("hl1-won", { k: { command: action.command, base: "" } }));
      for (const [name, value] of script.aliases) assert.equal(written.get(name), value, `${script.id}: alias ${name}`);
    }
    if (script.video) assert.match(script.video.id, /^[\w-]{11}$/, script.id);
  }
  const guide = hl1.presets.filter((preset) => preset.category === "guide");
  assert.ok(guide.length && guide.every((preset) => !preset.listed && preset.scripted), "в списке клавиатуры их нет, отмечены как скрипты");
});

test("Скрипты гайда: чувствительность берётся из config.cfg, auto_alias — из game.cfg", (t) => {
  const root = tempDir(t);
  const gameDir = path.join(root, "Half-Life/valve_WON");
  writeFile(path.join(gameDir, "config.cfg"), 'unbindall\r\nbind "w" "+forward"\r\nsensitivity "2.25"\r\n');
  const config = manifests.LAUNCH_CONFIG["hl1-won"];
  const p2p = hl1Guide.scriptById("pipe2pipe-script-v2");
  const saveload = hl1Guide.scriptById("simple-saveload-script");
  const applyBinds = (binds, previous) => gameBindsInstall.applyBinds(root, "hl1-won", config.gameDirectory, binds, { language: "english", previous });
  const first = {
    3: { command: p2p.actions[0].command, base: "" },
    8: { command: saveload.actions[0].command, base: "" },
  };
  applyBinds(first);
  const cfg = fs.readFileSync(path.join(gameDir, "userconfig.cfg"), "latin1");
  assert.doesNotMatch(cfg, /\{sensitivity\}/, "токен не попадает в игру");
  assert.match(cfg, /alias \S*sens "sensitivity 2\.25"/, "чувствительность игрока подставлена");
  const lines = cfg.split("\r\n");
  assert.equal(lines.find((line) => line.startsWith("alias auto_alias")), "alias auto_alias", "auto_alias пуст до нажатия скрипта");
  assert.match(cfg, /bind "8" "alias auto_alias \S+"/);
  const gameCfg = fs.readFileSync(path.join(gameDir, "game.cfg"), "latin1");
  assert.equal(gameCfg, "auto_alias\r\n");

  // Повторная запись не дублирует строку; чужой game.cfg сохраняется.
  fs.writeFileSync(path.join(gameDir, "game.cfg"), "echo hi\r\n", "latin1");
  applyBinds({ 8: { command: saveload.actions[0].command, base: "" } });
  applyBinds({ 8: { command: saveload.actions[0].command, base: "" } });
  assert.equal(fs.readFileSync(path.join(gameDir, "game.cfg"), "latin1"), "echo hi\r\nauto_alias\r\n");

  // Без config.cfg или с мусором вместо числа — значение по умолчанию.
  fs.writeFileSync(path.join(gameDir, "config.cfg"), 'sensitivity "2;quit"\r\n', "latin1");
  applyBinds({ 3: { command: p2p.actions[0].command, base: "" } }, first);
  assert.match(fs.readFileSync(path.join(gameDir, "userconfig.cfg"), "latin1"), /alias \S*sens "sensitivity 3"/);
  assert.equal(hl1Guide.cleanSensitivity("0"), "3");
  assert.equal(hl1Guide.cleanSensitivity("1.5"), "1.5");
});

test("Скрипты гайда: после трюка возвращаются FOV, прицел и инверсия мыши игрока", (t) => {
  const root = tempDir(t);
  const config = { engine: "goldsrc", gameDirectory: "Half-Life/valve_WON" };
  const gameDir = path.join(root, "Half-Life/valve_WON");
  fs.mkdirSync(gameDir, { recursive: true });
  fs.writeFileSync(path.join(gameDir, "config.cfg"), 'sensitivity "2"\r\nm_pitch "-0.022"\r\n', "latin1");
  const actionOf = (id) => hl1Guide.scriptById(id).actions[0].command;
  gameBindsInstall.applyBinds(root, "hl1-won", config.gameDirectory, {
    3: { command: actionOf("gauss-door-setup-script"), base: "" },
    4: { command: actionOf("semi-manual-xen-script"), base: "" },
    5: { command: actionOf("fully-automatic-xen-script"), base: "" },
  }, { language: "english", settings: { fov: 120 } });
  const cfg = fs.readFileSync(path.join(gameDir, "userconfig.cfg"), "latin1");
  assert.ok(fs.existsSync(path.join(gameDir, "hlsr-personalization.cfg")), "снимок для deffov создан");
  assert.ok(!cfg.split("\r\n").includes("exec hlsr-personalization.cfg"), "в файл игрока exec снимка не попадает");
  assert.doesNotMatch(cfg, /\{m_pitch\}|\{sensitivity\}/, "токены не попадают в игру");
  assert.doesNotMatch(cfg, /default_fov 110|bxt_cross 0"/, "значения автора гайда не возвращаются");
  assert.match(cfg, /alias deffov "exec hlsr-personalization\.cfg"/);
  assert.match(cfg, /alias defcross "exec hlsr-personalization\.cfg"/);
  assert.doesNotMatch(cfg, /m_pitch 0\.022/, "инверсия мыши не сбрасывается");
  assert.equal(cfg.match(/m_pitch -0\.022/g).length, 2, "оба скрипта Xen возвращают m_pitch игрока");

  // Без m_pitch в config.cfg или с мусором — значение Half-Life по умолчанию.
  assert.equal(hl1Guide.cleanPitch(undefined), "0.022");
  assert.equal(hl1Guide.cleanPitch("0"), "0.022");
  assert.equal(hl1Guide.cleanPitch("0.022;quit"), "0.022");
  assert.equal(hl1Guide.cleanPitch("-0.03"), "-0.03");
});

test("Скрипты гайда: наружу открываются только ссылки из гайда", () => {
  const video = hl1Guide.SCRIPTS.find((script) => script.video).video.id;
  const save = hl1Guide.SCRIPTS.find((script) => script.save).save;
  assert.ok(hl1Guide.isGuideLink(`${hl1Guide.SOURCE.repo}#readme`));
  assert.ok(hl1Guide.isGuideLink(save));
  assert.ok(hl1Guide.isGuideLink(`https://www.youtube.com/watch?v=${video}`));
  assert.ok(hl1Guide.isGuideLink(`https://www.youtube.com/watch?v=${video}&t=42`));
  assert.ok(!hl1Guide.isGuideLink("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), "чужое видео");
  assert.ok(!hl1Guide.isGuideLink(`https://www.youtube.com/watch?v=${video}&list=x`));
  assert.ok(!hl1Guide.isGuideLink("https://example.com/"));
  assert.ok(!hl1Guide.isGuideLink("file:///etc/passwd"));
  assert.ok(!hl1Guide.isGuideLink(null));
});

test("Скрипты гайда: русский перевод описаний есть у всех и сохраняет ссылки и код", () => {
  const links = (text) => [...String(text).matchAll(/\]\(([^)\s]+)\)/g)].map((m) => m[1]).sort();
  const code = (text) => [...String(text).matchAll(/`([^`]+)`/g)].map((m) => m[1]).sort();
  const ru = require("../src/shared/hl1-guide-ru.json");
  assert.deepEqual(Object.keys(ru.scripts).sort(), hl1Guide.SCRIPTS.map((script) => script.id).sort(), "перевод без лишних и пропущенных скриптов");
  for (const script of hl1Guide.SCRIPTS) {
    const item = hl1Guide.translationOf(script, "ru");
    assert.ok(item, `${script.id}: перевод устарел после импорта гайда`);
    const en = script.notes.concat(script.usage).join("\n");
    const text = item.notes.concat(item.usage).join("\n");
    assert.deepEqual(links(text), links(en), `${script.id}: ссылки`);
    assert.equal(code(text).length, code(en).length, `${script.id}: код`);
  }
  // Изменённый оригинал показывается без старого перевода.
  const changed = Object.assign({}, hl1Guide.SCRIPTS[0], { usage: "new text" });
  assert.equal(hl1Guide.translationOf(changed, "ru"), null);
  assert.equal(hl1Guide.translationOf(hl1Guide.SCRIPTS[0], "en"), null);
});

test("Скрипты гайда: данные совпадают с импортом зафиксированного README", { skip: !process.env.HLSR_GUIDE_README }, () => {
  const { convert } = require("./import-hl1-guide");
  const fresh = convert(fs.readFileSync(process.env.HLSR_GUIDE_README, "utf8"));
  assert.deepEqual(JSON.parse(JSON.stringify(fresh)), require("../src/shared/hl1-guide-data.json"));
});

test("Бинды Source: читаются config.cfg и autoexec.cfg, переназначения пишутся в autoexec.cfg", (t) => {
  const root = tempDir(t);
  const cfgDir = path.join(root, "ghostingmod/cfg");
  fs.mkdirSync(cfgDir, { recursive: true });
  fs.writeFileSync(path.join(cfgDir, "config.cfg"), 'unbindall\nbind "w" "+forward"\nbind "NUMLOCK" "pause"\nbind "SEMICOLON" "+use"\n');
  fs.writeFileSync(path.join(cfgDir, "autoexec.cfg"), 'bind mouse4 "save quick" //Клавиша сейва\r\nbind x "save quick; load quick"\r\n');
  const read = gameBindsInstall.readInstallBinds(root, "ghostingmod", "source");
  assert.equal(read.binds.w.command, "+forward");
  assert.equal(read.binds.NUMLOCK.command, "pause", "клавиши Source распознаются");
  assert.equal(read.binds[";"].command, "+use");
  assert.equal(read.binds.MOUSE4.source, "autoexec.cfg");
  assert.equal(read.binds.x.command, "save quick; load quick");

  const binds = { ";": { command: "+jump", base: "+use" }, RSHIFT: { command: "+speed", base: "" } };
  gameBindsInstall.applyBinds(root, "ghosting-renewed", "ghostingmod", binds);
  gameBindsInstall.applyBinds(root, "ghosting-renewed", "ghostingmod", binds);
  const autoexec = fs.readFileSync(path.join(cfgDir, "autoexec.cfg"), "utf8");
  assert.match(autoexec, /bind "semicolon" "\+jump"/, "Source не понимает «;» в имени клавиши");
  assert.match(autoexec, /bind "rshift" "\+speed"/);
  assert.equal(autoexec.split('"rshift"').length - 1, 1, "строка одна");
  assert.ok(autoexec.startsWith('bind mouse4 "save quick" //Клавиша сейва\r\n'), "своё не тронуто");
  assert.equal(fs.existsSync(path.join(cfgDir, "hlsr-binds.cfg")), false);
});

test("Ghosting: стоковый autoexec меняется на стандартный лаунчера, свой — нет", (t) => {
  const root = tempDir(t);
  const cfgDir = path.join(root, "ghostingmod/cfg");
  fs.mkdirSync(cfgDir, { recursive: true });
  const stock = "fov_desired 90\ncon_enable 1\ncl_showfps 1\ncl_showpos 1\nfps_max 300\n\ndemo_interplimit 4000\n\nexec gh_optimized.cfg";
  fs.writeFileSync(path.join(cfgDir, "autoexec.cfg"), `${stock}\nexec hlsr-personalization.cfg\n`);
  assert.equal(ghostingPersonalizationModule.ensureDefaultAutoexec(root), true);
  const text = fs.readFileSync(path.join(cfgDir, "autoexec.cfg"), "utf8");
  assert.match(text, /bind k "load quick01"/);
  assert.ok(text.endsWith("exec hlsr-personalization.cfg\r\n"), "строка exec лаунчера сохраняется");
  assert.equal(ghostingPersonalizationModule.ensureDefaultAutoexec(root), false, "второй раз уже не стоковый");

  fs.writeFileSync(path.join(cfgDir, "autoexec.cfg"), "fps_max 999\n");
  assert.equal(ghostingPersonalizationModule.ensureDefaultAutoexec(root), false);
  assert.equal(fs.readFileSync(path.join(cfgDir, "autoexec.cfg"), "utf8"), "fps_max 999\n");

  const fresh = tempDir(t);
  assert.equal(ghostingPersonalizationModule.ensureDefaultAutoexec(fresh), true, "нет файла — ставится стандартный");
  const preset = bindPresets.presetOf(bindPresets.catalogFor("ghosting-renewed"), "load quick01");
  assert.ok(preset, "бинды стандартного autoexec есть в заготовках");
});

test("Ghosting: удаление сейва под Linux через «save /», старый бинд в autoexec меняется", (t) => {
  const expected = process.platform === "linux" ? "save /; reload" : "save :; reload";
  assert.equal(bindPresets.GHOSTING_SAVE_DELETION, expected);
  assert.ok(bindPresets.presetOf(bindPresets.catalogFor("ghosting-renewed"), expected), "заготовка удаления сейва");
  const fresh = tempDir(t);
  ghostingPersonalizationModule.ensureDefaultAutoexec(fresh);
  assert.ok(fs.readFileSync(path.join(fresh, "ghostingmod/cfg/autoexec.cfg"), "utf8").includes(`bind h "${expected}"\r\n`));

  const root = tempDir(t);
  const file = path.join(root, "ghostingmod/cfg/autoexec.cfg");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const own = "fps_max 999\r\n\tbind h \"save :; reload\"\r\nbind j \"save :; reload; echo 1\"\r\nexec hlsr-personalization.cfg\r\n";
  fs.writeFileSync(file, own);
  assert.equal(ghostingPersonalizationModule.ensureDefaultAutoexec(root, { platform: "win32" }), false);
  assert.equal(fs.readFileSync(file, "utf8"), own, "под Windows файл не меняется");
  assert.equal(ghostingPersonalizationModule.ensureDefaultAutoexec(root, { platform: "linux" }), false);
  assert.equal(fs.readFileSync(file, "utf8"), own.replace("\tbind h \"save :; reload\"", `\tbind h "${expected}"`),
    "меняется только бинд лаунчера, отступы и окончания строк те же");
});

test("HL1 Linux: курсор RInput в меню патчится, только когда включён", async (t) => {
  const calls = [];
  const statuses = ["waiting", "waiting", "patched"];
  const rig = rinputRig(t, null, { rinputMenuPatcher: (name) => { calls.push(name); return statuses.shift(); } });
  const infos = [];
  rig.service.logger.info = (m) => infos.push(m);
  await rig.service.startGoldSrc("hl1-won", { installPath: rig.root }, config, { rinputMenuFix: true });
  await rig.service.rinputStart;
  assert.deepEqual(calls, ["hl.exe", "hl.exe", "hl.exe"], "ждём, пока RInput поставит перехваты");
  assert.ok(infos.some((m) => /курсор RInput в меню исправлен/.test(m)));

  const off = rinputRig(t, null);
  await off.service.startGoldSrc("hl1-won", { installPath: off.root }, config, null);
  await off.service.rinputStart;
  assert.deepEqual(off.warnings, [], "по умолчанию память игры не трогаем");

  const other = rinputRig(t, null, { rinputMenuPatcher: () => "unsupported" });
  await other.service.startGoldSrc("hl1-won", { installPath: other.root }, config, { rinputMenuFix: true });
  await other.service.rinputStart;
  assert.ok(other.warnings.some((m) => /незнакомая версия RInput/.test(m)));
  assert.equal(other.launches.length, 2, "игра и RInput работают и без патча");

  const slow = rinputRig(t, null, { rinputMenuPatcher: () => "waiting", rinputPatchWaitMs: 5 });
  await slow.service.startGoldSrc("hl1-won", { installPath: slow.root }, config, { rinputMenuFix: true });
  await slow.service.rinputStart;
  assert.ok(slow.warnings.some((m) => /не перехватил курсор/.test(m)));
});

// Память hl.exe в миниатюре: RInput.dll, сдвинутая загрузчиком, и user32 с экспортами.
function fakeGame({ hooked = true, hookBytes } = {}) {
  const B = 0x642c0000;
  const U = 0x7b5b0000;
  const rinput = Buffer.alloc(0x13000);
  (hookBytes || rinputMenuFix.originalHook(B)).copy(rinput, rinputMenuFix.HOOK);
  rinput.fill(0xcc, 0x154a, 0x1550);
  const user32 = Buffer.alloc(0x3000);
  user32.writeUInt32LE(0x80, 0x3c);
  user32.writeUInt32LE(0x1000, 0x80 + 0x78);
  user32.writeUInt32LE(0x300, 0x80 + 0x7c);
  [[20, 2], [24, 2], [28, 0x1100], [32, 0x1120], [36, 0x1140]].forEach(([off, v]) => user32.writeUInt32LE(v, 0x1000 + off));
  user32.writeUInt32LE(0x2000, 0x1100);
  user32.writeUInt32LE(0x2100, 0x1104);
  user32.writeUInt32LE(0x1200, 0x1120);
  user32.writeUInt32LE(0x1220, 0x1124);
  user32.writeUInt16LE(1, 0x1142);
  user32.write("GetCursorInfo\0", 0x1200, "latin1");
  user32.write("GetCursorPos\0", 0x1220, "latin1");
  if (hooked) {
    user32[0x2100] = 0xe9;
    user32.writeInt32LE(B + rinputMenuFix.HOOK - (U + 0x2105), 0x2101);
  } else user32.write("8bff558bec", 0x2100, "hex");
  const regions = [[B, rinput], [U, user32]];
  const at = (address) => {
    const [start, buf] = regions.find(([start, buf]) => address >= start && address < start + buf.length);
    return [buf, address - start];
  };
  const writes = [];
  const fakeFs = {
    readdirSync: () => ["self", "4242"],
    readFileSync: (file) => {
      if (file === "/proc/4242/comm") return "hl.exe\n";
      if (file === "/proc/4242/maps") return [
        "642c0000-642c1000 r-xp 00000000 00:21 1 /games/hl1-won/RInput/RInput.dll",
        "642d1000-642d3000 rwxp 0000f000 00:21 1 /games/hl1-won/RInput/RInput.dll",
        "7b5b0000-7b678000 r-xp 00000000 00:21 2 /proton/lib/wine/i386-windows/user32.dll",
      ].join("\n");
      throw new Error(`нет файла ${file}`);
    },
    openSync: () => 7,
    closeSync: () => {},
    readSync: (fd, buf, off, len, pos) => { const [src, i] = at(pos); src.copy(buf, off, i, i + len); return len; },
    writeSync: (fd, buf, off, len, pos) => { writes.push(pos); const [dst, i] = at(pos); buf.copy(dst, i, off, off + len); return len; },
  };
  return { B, U, rinput, fakeFs, writes };
}

function jumpTarget(buf, offset, address) { return (address + 5 + buf.readInt32LE(offset + 1)) >>> 0; }

test("HL1 Linux: патч курсора RInput пишет переход в обход только знакомой DLL", async () => {
  const game = fakeGame();
  const { B, U, rinput } = game;
  assert.equal(await rinputMenuFix.patchRunningGame("hl.exe", game.fakeFs), "patched");
  assert.equal(rinput.subarray(0x1530, 0x1532).toString("hex"), "eb18", "хук начинается с короткого перехода");
  assert.equal(rinput.subarray(0x1547, 0x154a).toString("hex"), "c20400", "ret 4 исходного хука цел");
  assert.equal(rinput[0x154a], 0xe9);
  assert.equal(jumpTarget(rinput, 0x154a, B + 0x154a), B + rinputMenuFix.CAVE);
  const cave = rinputMenuFix.buildCave(B, U + 0x2000);
  assert.ok(rinput.subarray(rinputMenuFix.CAVE, rinputMenuFix.CAVE + cave.length).equals(cave));
  assert.equal(cave.readUInt32LE(12), U + 0x2000, "вызывается GetCursorInfo из user32");
  assert.equal(jumpTarget(cave, 39, B + rinputMenuFix.CAVE + 39), B + rinputMenuFix.TRAMPOLINE, "курсор показан: настоящая GetCursorPos");
  assert.equal(jumpTarget(cave, cave.length - 5, B + rinputMenuFix.CAVE + cave.length - 5), B + 0x1534, "курсор скрыт: дальше код RInput");
  assert.ok(game.writes.indexOf(B + 0x1530) === game.writes.length - 1, "переход на патч ставится последним");
  assert.equal(await rinputMenuFix.patchRunningGame("hl.exe", game.fakeFs), "already");

  const early = fakeGame({ hooked: false });
  assert.equal(await rinputMenuFix.patchRunningGame("hl.exe", early.fakeFs), "waiting", "RInput ещё не перехватил курсор");
  assert.deepEqual(early.writes, []);

  const foreign = fakeGame({ hookBytes: Buffer.alloc(26, 0x90) });
  assert.equal(await rinputMenuFix.patchRunningGame("hl.exe", foreign.fakeFs), "unsupported");
  assert.deepEqual(foreign.writes, [], "чужую версию RInput не трогаем");
});

const runsScan = require("../src/main/game-delivery/runs-scan");

test("Импорт cfg: цепочка exec спрашивает каждый файл по разу, сначала верхний уровень", () => {
  const root = { name: "main.cfg", text: 'exec "binds.cfg"\r\nalias +x "exec scripts/run.cfg"\r\nexec config.cfg\r\nexec binds.cfg\r\n' };
  assert.deepEqual(cfgImport.nextWanted({ engine: "goldsrc", root }), { key: "binds.cfg", name: "binds.cfg", from: "main.cfg" });
  const files = { "binds.cfg": { name: "Binds.cfg", text: "exec more.cfg\nbind k +jump" } };
  assert.equal(cfgImport.nextWanted({ engine: "goldsrc", root, files }).key, "scripts/run.cfg", "сначала файлы, на которые exec указывает сам конфиг");
  assert.equal(cfgImport.nextWanted({ engine: "goldsrc", root, files, topLevelOnly: true }).key, "more.cfg", "exec в alias не нужен импорту только биндов");
  const all = Object.assign({}, files, { "scripts/run.cfg": { name: "run.cfg", text: "+jump" } });
  assert.equal(cfgImport.nextWanted({ engine: "goldsrc", root, files: all, missing: ["more.cfg"] }), null, "config.cfg игры и отказ не спрашиваются");
  // GoldSrc не выполняет файлы без .cfg, Source дописывает расширение.
  assert.equal(cfgImport.execTarget("jump", "goldsrc"), null);
  assert.equal(cfgImport.execTarget("jump", "source").key, "jump.cfg");
  assert.equal(cfgImport.execTarget("../evil.cfg", "goldsrc"), null);
});

test("Импорт cfg: файлы под своими именами, цикл exec отключён, повторы в отчёте", () => {
  const built = cfgImport.buildImport({
    engine: "goldsrc",
    root: { name: "main.cfg", text: "fps_max 100\nexec a.cfg\nexec b.cfg\nbind f +use\nalias go \"exec a.cfg\"\n" },
    files: {
      "a.cfg": { name: "a.cfg", text: "bind f +attack\nexec main.cfg\nfps_max 250\n" },
      "b.cfg": { name: "b.cfg", text: "bind f +attack\nexec main.cfg\nfps_max 250\n" },
    },
    missing: ["c.cfg"],
  });
  const byFile = Object.fromEntries(built.outputs.map((item) => [item.file, item.text]));
  assert.deepEqual(Object.keys(byFile), ["main.cfg", "a.cfg", "b.cfg"], "имена как у игрока");
  assert.deepEqual(built.execs, ["main.cfg"]);
  assert.match(byFile["main.cfg"], /^exec a\.cfg$/m, "exec не переписывается");
  assert.match(byFile["main.cfg"], /^exec b\.cfg$/m);
  assert.match(byFile["main.cfg"], /alias go "exec a\.cfg"/);
  assert.match(byFile["a.cfg"], /^\/\/ HLSR: exec loop: exec main\.cfg$/m, "a.cfg → main.cfg крутился бы бесконечно");
  assert.deepEqual(built.report.cycles, [["a.cfg", "main.cfg"], ["b.cfg", "main.cfg"]]);
  assert.deepEqual(built.report.duplicateFiles, [["a.cfg", "b.cfg"]]);
  const dup = Object.fromEntries(built.report.duplicates.map((item) => [`${item.kind} ${item.name}`, item]));
  assert.deepEqual(Object.keys(dup).sort(), ["bind f", "cvar fps_max"]);
  assert.equal(dup["bind f"].value, "+use", "действует последнее");
  assert.equal(dup["cvar fps_max"].value, "250");
  // Плоский текст — порядок выполнения: файл цепочки на месте своего exec.
  const flat = goldsrcBinds.withImport(null, built.flatText);
  assert.equal(flat.binds.f.command, "+use");
});

test("HL1: скрипты находятся и внутри составных биндов конфига", () => {
  assert.deepEqual(goldsrcBinds.commandsOf("save oarEnt; bind mouse4 +tau; say TAO"),
    ["save oarEnt", "bind mouse4 +tau", "+tau", "say TAO"]);
});

test("HL1: скрипт за цепочкой alias игрока находится по шагам перебинда", () => {
  const aliases = {
    re4: "re41",
    re41: "bind f fps4; say 4fps; alias re4 re42",
    re42: "bind f obbo1000; say obbo1000; alias re4 re41",
    obbo1000: "+use;w 8;-use;+jump;wait;-jump",
    "+hold": "fps_max 20",
    "-hold": "fps_max 100",
  };
  const parts = goldsrcBinds.expandCommand("re4", aliases);
  assert.ok(parts.includes("bind f obbo1000"));
  assert.ok(parts.includes("obbo1000"));
  assert.ok(goldsrcBinds.expandCommand("+HOLD", aliases).includes("fps_max 100"), "у +x раскрывается и -x, регистр не важен");
  const looped = { a: "b", b: "a" };
  assert.deepEqual(goldsrcBinds.expandCommand("a", looped), ["a", "b", "a"], "цикл alias не зацикливает разбор");
});

test("HL1: в Scriptless скриптом считается и одно слово, если за ним скрипт", () => {
  const hl1 = bindPresets.catalogFor("hl1-won");
  const aliases = {
    fps4: "fps_max 4; w; fps_max 100",
    fps666: "fps_max 66.66666",
    fix: "-jump;-duck;fps_max 100",
    re5: "re51",
    re51: "bind tab fps666; alias re5 re52",
    re52: "bind tab fps4; alias re5 re51",
    stepper: "fps666",
    "+myduck": "+duck",
    "-myduck": "-duck",
    _saveload2: "w 27; save quick; load quick",
  };
  assert.equal(bindPresets.isScripted(hl1, "fps4", aliases), true, "alias с несколькими командами");
  assert.equal(bindPresets.isScripted(hl1, "FIX", aliases), true, "имя alias без учёта регистра");
  assert.equal(bindPresets.isScripted(hl1, "re5", aliases), true, "переключатель, который переопределяют другие alias");
  assert.equal(bindPresets.isScripted(hl1, "+myduck", aliases), true, "alias на +команду в Scriptless запрещён");
  assert.equal(bindPresets.isScripted(hl1, "alias auto_alias _saveload2", aliases), true, "alias, который запустит скрипт");
  assert.equal(bindPresets.isScripted(hl1, "bind f fps4", aliases), true, "перебинд на скрипт");
  assert.equal(bindPresets.isScripted(hl1, "bxt_autojump 1", aliases), true, "bxt_autojump — только Scripted");
  assert.equal(bindPresets.isScripted(hl1, "fps666", aliases), false, "alias одной команды разрешён");
  assert.equal(bindPresets.isScripted(hl1, "stepper", aliases), false, "цепочка alias одной команды разрешена");
  assert.equal(bindPresets.isScripted(hl1, "fps4"), false, "без alias игрока одно слово — не скрипт");
});

test("HL1: скрипты гайда на клавиатуре подписаны как в гайде, по-английски", () => {
  const catalog = bindPresets.catalogFor("hl1-won");
  for (const command of ["+usespam", "+tau", "obboshoot", "4fpsbind"]) {
    const preset = bindPresets.presetOf(catalog, command);
    assert.equal(preset.label.ru, preset.label.en, command);
    assert.ok(preset.guide, `${command}: связан со скриптом гайда`);
  }
  assert.equal(bindPresets.presetOf(catalog, "+bxt_tas_jumpbug").label.ru, "Джампбаг (BXT)", "общие команды BXT не переименованы");
});

test("HL1: демо ищутся и в языковой папке мода", (t) => {
  const root = fakeInstall(tempDir(t));
  writeFile(path.join(root, "Half-Life/valve_WON/a.dem"), "x");
  writeFile(path.join(root, "Half-Life/valve_won_russian/b.dem"), "x");
  writeFile(path.join(root, "Half-Life/valve_WON_extra/c.dem"), "x");
  const dirs = runsScan.runDirs(root, manifests.LOCAL_CONFIG["hl1-won"].runsPath);
  assert.deepEqual(dirs, ["Half-Life/valve_won_russian", "Half-Life/valve_WON"]);
  const listed = runsScan.scanMany(root, dirs, ".dem");
  assert.equal(listed.status, "ok");
  assert.equal(listed.path, path.join(root, "Half-Life/valve_won_russian"), "«Открыть папку» ведёт в языковую папку");
  assert.deepEqual(listed.items.map((item) => item.name).sort(), ["a.dem", "b.dem"]);
});

// Перенос конфига игрока целиком: файлы, которые движок выполняет сам,
// кодировки Блокнота, exec файлов игры в одной строке с другими командами,
// персонализация и переназначения лаунчера поверх импорта.
const importAdopt = require("../src/shared/goldsrc-import-adopt");

test("Импорт cfg: userconfig.cfg игрока из config.cfg спрашивается и подключается", () => {
  const config = "unbindall\r\nbind \"t\" \"exec userconfig.cfg; say Tero0\"\r\nbind \"-\" \"p2pre\"\r\nexec userconfig.cfg\r\n";
  const root = { name: "config.cfg", text: config };
  const want = cfgImport.nextWanted({ engine: "goldsrc", root });
  assert.equal(want && want.name, "userconfig.cfg", "движок выполнит userconfig.cfg игрока после config.cfg");
  const files = { "userconfig.cfg": { name: "userconfig.cfg", text: "alias p2pre \"say pre\"\r\nexec config.cfg; echo after\r\n" } };
  assert.equal(cfgImport.nextWanted({ engine: "goldsrc", root, files }), null);
  const built = cfgImport.buildImport({ engine: "goldsrc", root, files });
  const byFile = Object.fromEntries(built.outputs.map((item) => [item.file, item.text]));
  assert.deepEqual(Object.keys(byFile), ["config.cfg", "userconfig.cfg"], "файлы ложатся на свои места");
  assert.deepEqual(built.execs, [], "оба файла движок выполняет сам");
  assert.match(byFile["config.cfg"], /^exec userconfig\.cfg\r?$/m);
  assert.match(byFile["config.cfg"], /bind "t" "exec userconfig\.cfg; say Tero0"/, "exec в бинде ведёт к userconfig игрока");
  assert.match(byFile["userconfig.cfg"], /^ ?echo after +\/\/ HLSR: exec config\.cfg\r?$/m, "exec config.cfg убран, echo остался");
  assert.equal(goldsrcBinds.withImport(null, built.flatText).aliases.p2pre, "say pre");
  // Без userconfig.cfg игрока его exec не выполнит userconfig.cfg лаунчера
  // (а тот — снова импорт); exec в бинде остаётся как в игре.
  const alone = cfgImport.buildImport({ engine: "goldsrc", root, missing: ["userconfig.cfg"] }).outputs[0].text;
  assert.match(alone, /^\/\/ HLSR: not imported: exec userconfig\.cfg\r?$/m);
  assert.match(alone, /bind "t" "exec userconfig\.cfg; say Tero0"/);
  const unasked = cfgImport.buildImport({ engine: "goldsrc", root }).outputs[0].text;
  assert.match(unasked, /^\/\/ HLSR: not imported: exec userconfig\.cfg\r?$/m, "и когда файл не спрашивали");
  // Текст без цепочки (importFileText строкой) тоже не вызывает userconfig.cfg.
  assert.match(goldsrcBinds.importFileText("exec userconfig.cfg\nexec autoexec.cfg\n"), /^\/\/ HLSR: exec userconfig\.cfg\n\/\/ HLSR: exec autoexec\.cfg\n$/);
});

test("Импорт cfg: к config.cfg просится autoexec.cfg, свои имена файлов игрока сохраняются", () => {
  assert.deepEqual(cfgImport.companionsOf(["Config.cfg"]), [{ name: "autoexec.cfg", from: "config.cfg", before: true }]);
  assert.deepEqual(cfgImport.companionsOf(["config.cfg", "autoexec.cfg"]), []);
  assert.deepEqual(cfgImport.companionsOf(["userconfig.cfg"]), []);
  const { root, files } = cfgImport.rootOfPicked([
    { name: "autoexec.cfg", text: "alias a1 \"say a\"\n" },
    { name: "config.cfg", text: "bind q \"exec autoexec.cfg\"\nexec userconfig.cfg\n" },
  ]);
  assert.deepEqual(Object.keys(files), ["autoexec.cfg", "config.cfg"], "имена как у игрока");
  assert.equal(root.text, 'exec "autoexec.cfg"\nexec "config.cfg"');
  assert.equal(root.virtual, true, "своего файла у нескольких выбранных нет");
  assert.deepEqual(Object.keys(cfgImport.rootOfPicked([{ name: "a.cfg", text: "" }, { name: "A.cfg", text: "" }]).files),
    ["a.cfg", "a-2.cfg"], "одинаковые имена не затирают друг друга");
  const want = cfgImport.nextWanted({ engine: "goldsrc", root, files });
  assert.equal(want && want.name, "userconfig.cfg", "autoexec.cfg уже выбран, спрашивается только userconfig.cfg");
  const built = cfgImport.buildImport({ engine: "goldsrc", root, files, missing: ["userconfig.cfg"] });
  const config = built.outputs.find((item) => /bind q/.test(item.text)).text;
  assert.match(config, /bind q "exec autoexec\.cfg"/);
  assert.match(config, /^\/\/ HLSR: not imported: exec userconfig\.cfg$/m);
  assert.deepEqual(built.outputs.map((item) => item.file), ["autoexec.cfg", "config.cfg"]);
  assert.deepEqual(built.execs, []);
  const own = cfgImport.buildImport({ engine: "goldsrc", ...cfgImport.rootOfPicked([
    { name: "binds.cfg", text: "bind k +jump\n" }, { name: "scripts.cfg", text: "alias x y\n" }]) });
  assert.deepEqual(own.execs, ["binds.cfg", "scripts.cfg"], "свои файлы подключаются из userconfig.cfg по порядку");
});

test("Импорт cfg: повторный импорт сохранённого hlsr-import.cfg спрашивает файлы его цепочки", () => {
  const root = { name: "hlsr-import.cfg", text: "// Imported by HLSR Launcher\r\nexec hlsr-import-jump.cfg\r\nexec hlsr-binds.cfg\r\n" };
  const want = cfgImport.nextWanted({ engine: "goldsrc", root });
  assert.equal(want && want.name, "hlsr-import-jump.cfg");
  const built = cfgImport.buildImport({ engine: "goldsrc", root, files: { "hlsr-import-jump.cfg": { name: "hlsr-import-jump.cfg", text: "bind j +jump\r\n" } } });
  assert.match(built.outputs[0].text, /^\/\/ HLSR: exec hlsr-binds\.cfg\r?$/m, "файлы лаунчера из хвоста не выполняются раньше времени");
  assert.deepEqual(built.outputs.map((item) => item.file), ["player.cfg", "jump.cfg"], "имена прошлых версий без hlsr-import");
  assert.match(built.outputs[0].text, /^exec jump\.cfg\r?$/m);
  assert.equal(goldsrcBinds.withImport(null, built.flatText).binds.j.command, "+jump");
});

test("Импорт cfg: BOM и UTF-16 Блокнота, exec файлов игры посреди строки", () => {
  assert.equal(goldsrcBinds.cfgText("\xef\xbb\xbfbind a +jump"), "bind a +jump");
  const utf16 = (text) => String.fromCharCode(...Buffer.from(text, "utf16le"));
  assert.equal(goldsrcBinds.cfgText("\xff\xfe" + utf16("bind a +jump\r\n")), "bind a +jump\r\n");
  assert.equal(goldsrcBinds.cfgText(utf16("alias x \"+use\"\r\n")), "alias x \"+use\"\r\n", "UTF-16 без BOM");
  const big = "\xfe\xff" + String.fromCharCode(...Buffer.from("say привет", "utf16le").swap16());
  assert.equal(Buffer.from(goldsrcBinds.cfgText(big), "latin1").toString("utf8"), "say привет", "UTF-16 BE, кириллица — байтами UTF-8");
  assert.equal(goldsrcBinds.cfgText("say \xcf\xf0\xe8\xe2\xe5\xf2"), "say \xcf\xf0\xe8\xe2\xe5\xf2", "cp1251 не трогается");
  assert.deepEqual(goldsrcBinds.parseCfg(goldsrcBinds.importFileText("\xef\xbb\xbfbind a +jump"))[0], ["bind", "a", "+jump"]);
  const text = goldsrcBinds.importFileText('echo a; exec "config.cfg" ; bind x "exec config.cfg; say hi" // c\r\nexec valve.rc\r\nexec hlsr-import-x.cfg\r\n');
  assert.equal(text, 'echo a; bind x "exec config.cfg; say hi"  // HLSR: exec "config.cfg" // c\r\n// HLSR: exec valve.rc\r\nexec hlsr-import-x.cfg\r\n');
});

test("HL1: отрицательный зазор прицела пишет BXT, Ghosting — не меньше 0", () => {
  const adopted = importAdopt.adoptImported("bxt_cross 1\nbxt_cross_gap -4\n", {});
  assert.deepEqual(adopted.skipped, []);
  assert.equal(adopted.personalization.crossGap, -4);
  const settings = launchArgs.sanitizeSettings({ personalization: adopted.personalization }).settings;
  assert.equal(settings.personalization.crossGap, -4, "сохранение не обрезает зазор");
  assert.equal(cfgValues(goldsrcPersonalization.toCfgLines(settings)).bxt_cross_gap, "-4");
  assert.match(ghostingPersonalizationModule.toCfgLines(settings.personalization), /^gh_cross_gap "0"$/m);
  assert.equal(launchArgs.sanitizeSettings({ personalization: { crossGap: -99 } }).settings.personalization.crossGap, -30);
});

test("Импорт HL1: смещения оружия, панели BXT и остальные значения генератора читаются из файла", () => {
  const text = [
    "bxt_viewmodel_ofs_forward -2", "bxt_viewmodel_ofs_right 2", "bxt_viewmodel_ofs_up 99", "bxt_remove_viewmodel 1",
    "bxt_viewmodel_bob_angled 1", "bxt_viewmodel_disable_idle 1", "bxt_viewmodel_disable_equip 1",
    "bxt_hud_jumpdistance 1", "bxt_hud_precision 3", "bxt_hud_incorrect_fps_indicator 0", 'bxt_hud_timer_anchor "0.25 0.5"',
  ].join("\n");
  const { bxt, skipped } = importAdopt.adoptImported(text, {});
  assert.equal(bxt.viewmodelForward, -2);
  assert.equal(bxt.viewmodelRight, 2);
  assert.equal(bxt.viewmodelUp, undefined, "вне -20..20 не берётся");
  assert.deepEqual(skipped, ["bxt_viewmodel_ofs_up 99"]);
  assert.deepEqual([bxt.removeViewmodel, bxt.viewmodelBobAngled, bxt.viewmodelNoIdle, bxt.viewmodelNoEquip], [true, true, true, true]);
  assert.deepEqual([bxt.jumpdistance, bxt.precision, bxt.incorrectFps, bxt.timerX, bxt.timerY], [true, 3, false, 25, 50]);
});

test("Импорт HL1: переназначения лаунчера снимаются только с клавиш из конфига", () => {
  const keys = goldsrcBinds.importTouchedKeys('unbindall\nbind "MOUSE4" "+tau"\nunbind f\nalias x "bind k +use"\n');
  assert.deepEqual([...keys].sort(), ["MOUSE4", "f"], "unbindall и бинды внутри alias не в счёт");
});

test("Импорт HL1: прицел, HUD и поле зрения из конфига переходят в персонализацию", () => {
  const text = [
    "default_fov 90", "bxt_hud_speedometer 0", "bxt_hud_jumpspeed 1", "bxt_viewmodel_fov 100",
    "bxt_cross 1", 'bxt_cross_color "255 0 0"', "bxt_cross_size 6", "bxt_cross_outline 2", 'bxt_cross_dot_color ""',
    "crosshair 0", 'bxt_hud_color "0 160 255"', "bxt_cross_gap 99",
    'alias zoom "default_fov 40"', "bxt_hud_timer 1", "bxt_hud_timer 0",
  ].join("\n");
  const result = importAdopt.adoptImported(text, { personalization: { crossPreset: "standard" }, bxt: { fov: 70, timer: true } });
  assert.deepEqual(result.bxt, { fov: 90, timer: false, speedometer: false, jumpspeed: true, viewmodelFov: 100, hideCrosshair: false }, "последнее значение главнее, alias не в счёт");
  const p = result.personalization;
  assert.equal(p.crossEnabled, true);
  assert.equal(p.crossPreset, "custom");
  assert.deepEqual(p.crossColor, { r: 255, g: 0, b: 0 });
  assert.equal(p.crossSize, 6);
  assert.equal(p.crossOutline, true);
  assert.equal(p.crossOutlineThickness, 2);
  assert.equal(p.crossDotColor, null);
  assert.equal(p.crossHideDefault, true);
  assert.deepEqual([p.hudEnabled, p.hudColor], [true, { r: 0, g: 160, b: 255 }]);
  assert.deepEqual(result.skipped, ["bxt_cross_gap 99"], "вне пределов лаунчера");
  // Без прицела BXT штатный прячет отдельный флаг.
  const bare = importAdopt.adoptImported("crosshair 0\ndefault_fov 150\n", { bxt: { fov: 100 } });
  assert.deepEqual(bare.skipped, ["default_fov 150"]);
  assert.deepEqual(bare.adopted, ["crosshair"]);
  assert.deepEqual(bare.bxt, { fov: 100, hideCrosshair: true });
  assert.equal(bare.personalization.crossEnabled, undefined, "прицел BXT не включается");
  const bareCfg = cfgValues(goldsrcPersonalization.toCfgLines({ bxt: bare.bxt }));
  assert.deepEqual([bareCfg.crosshair, bareCfg.bxt_cross], ["0", "0"]);
  const shown = importAdopt.adoptImported("crosshair 1\n", { bxt: { hideCrosshair: true } });
  assert.equal(shown.bxt.hideCrosshair, false, "crosshair 1 снимает флаг");
  // Результат проходит через настройки лаунчера без потерь.
  const written = goldsrcPersonalization.toCfgLines({ personalization: p, bxt: result.bxt });
  assert.match(written, /^default_fov "90"$/m);
  assert.match(written, /^bxt_cross_color "255 0 0"$/m);
  assert.match(written, /^bxt_cross_outline "2"$/m);
  assert.match(written, /^crosshair "0"$/m);
  assert.match(written, /^bxt_hud_speedometer "0"$/m);
});

// Запуск не пишет файл игрока: только переход с 0.4.8 или создание отсутствующего.
const PREPARE_SERVICES = [["Windows", WindowsLaunchService], ["Linux", LinuxLaunchService]];
const hl1Default = require("../src/main/game-delivery/hl1-default-userconfig");

for (const [name, Service] of PREPARE_SERVICES) {
  const service = () => new Service({ delivery: {}, steamLocator: {}, launcherVersion: "0.4.9" });

  test(`HL1 ${name}: запуск не меняет файл игрока и не создаёт hlsr-*.cfg`, (t) => {
    const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
    const mod = path.join(root, "Half-Life/valve_WON");
    const own = 'default_fov "110"\r\ncrosshair "0"\r\n';
    writeFile(path.join(mod, "userconfig.cfg"), own);
    writeFile(path.join(root, ".hlsr/config-setup.json"), '{ "migrated": "0.4.9" }');
    const settings = launchArgs.defaultSettings();
    settings.bxt = Object.assign({}, settings.bxt, { fov: 70 });

    service().prepareConfigs("hl1-won", { installPath: root }, config, settings);

    assert.equal(fs.readFileSync(path.join(mod, "userconfig.cfg"), "latin1"), own);
    assert.deepEqual(fs.readdirSync(mod).filter((file) => /^hlsr-/i.test(file)), []);
  });

  test(`HL1 ${name}: скрипт с auto_alias добавляет строку в game.cfg, файл игрока тот же`, (t) => {
    const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
    const mod = path.join(root, "Half-Life/valve_WON");
    const own = 'default_fov "110"\r\n';
    writeFile(path.join(mod, "userconfig.cfg"), own);
    writeFile(path.join(root, ".hlsr/config-setup.json"), '{ "migrated": "0.4.9" }');
    const saveload = hl1Guide.scriptById("simple-saveload-script");
    const settings = { binds: { 8: { command: saveload.actions[0].command, base: "" } } };

    service().prepareConfigs("hl1-won", { installPath: root }, config, settings);
    service().prepareConfigs("hl1-won", { installPath: root }, config, settings);

    assert.equal(fs.readFileSync(path.join(mod, "game.cfg"), "latin1"), "auto_alias\r\n");
    assert.equal(fs.readFileSync(path.join(mod, "userconfig.cfg"), "latin1"), own);
    assert.deepEqual(fs.readdirSync(mod).filter((file) => /^hlsr-/i.test(file)), []);
  });

  test(`HL1 ${name}: установка 0.4.8 при запуске переходит в один файл`, (t) => {
    const root = fakeInstall(path.join(tempDir(t), "hl1-won"));
    const mod = path.join(root, "Half-Life/valve_WON");
    const settings = launchArgs.defaultSettings();
    writeFile(path.join(mod, "userconfig.cfg"), `${hl1Default.TEXT}exec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n`, undefined);
    writeFile(path.join(mod, "hlsr-personalization.cfg"), goldsrcPersonalization.toCfgLines(settings));
    writeFile(path.join(mod, "hlsr-binds.cfg"), goldsrcBinds.toCfgLines({}, { engine: "goldsrc", aliases: [] }));

    service().prepareConfigs("hl1-won", { installPath: root }, config, settings);

    assert.ok(fs.existsSync(path.join(root, ".hlsr/config-setup.json")));
    assert.equal(JSON.parse(fs.readFileSync(path.join(root, ".hlsr/config-setup.json"), "utf8")).migrated, "0.4.9");
    assert.doesNotMatch(fs.readFileSync(path.join(mod, "userconfig.cfg"), "latin1"), /exec\s+hlsr-/i);
  });
}

test("бинды по нажатию: клавиша зовёт exec (и через alias) — его бинды видны как runtime", () => {
  const gb = require("../src/shared/game-binds");
  const files = {
    "config.cfg": 'unbindall\nbind k "exec tc_n.cfg"\nbind t "tog"\nalias tog "tog_on"\nalias tog_on "bind 7 +duck; alias tog tog_off"\nbind 8 +attack\nexec autoexec.cfg\nbind l "exec loop.cfg"',
    "autoexec.cfg": "bind 6 +use",
    "tc_n.cfg": 'bind 9 "say tc"\nbind 0 "exec tc_off.cfg"\nbind 8 +attack\nexec sub/more.cfg\nexec autoexec.cfg',
    "sub/more.cfg": 'bind "-" kill',
    "loop.cfg": "exec loop.cfg\nbind 5 +reload",
  };
  const read = gb.readBinds((name) => (name in files ? files[name] : null), ["config.cfg"]);
  const runtime = read.runtime;
  assert.deepEqual(runtime["9"], [{ command: "say tc", file: "tc_n.cfg", trigger: "k" }]);
  assert.deepEqual(runtime["0"], [{ command: "exec tc_off.cfg", file: "tc_n.cfg", trigger: "k" }]);
  assert.deepEqual(runtime["-"], [{ command: "kill", file: "sub/more.cfg", trigger: "k" }], "вложенный exec");
  assert.deepEqual(runtime["7"], [{ command: "+duck", file: null, trigger: "t" }], "bind прямо в alias");
  assert.deepEqual(runtime["5"], [{ command: "+reload", file: "loop.cfg", trigger: "l" }], "файл, зовущий сам себя, читается раз");
  assert.equal(runtime["8"], undefined, "бинд, совпадающий со стартовым, не считается");
  assert.equal(runtime["6"], undefined, "стартовый файл (autoexec) уже учтён");
  assert.equal(read.binds["9"], undefined, "на старте 9 пустая");
  // Импорт ещё не сохранён: прежние runtime с диска остаются у нетронутых клавиш,
  // а клавиша, которую импорт перебиндил, больше их не даёт.
  assert.deepEqual(gb.withImport(read, "bind q +jump").runtime, runtime);
  const rebound = gb.withImport(read, "bind k +use").runtime;
  assert.equal(rebound["9"], undefined, "k перебинжена импортом");
  assert.deepEqual(rebound["7"], runtime["7"], "t не тронута");
});
