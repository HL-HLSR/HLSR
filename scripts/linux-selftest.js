"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { EventEmitter } = require("events");

const ProtonManager = require("../src/main/linux/ProtonManager");
const ProtonRunner = require("../src/main/linux/ProtonRunner");
const ToolService = require("../src/main/game-delivery/ToolService");
const LiveSplitFixes = require("../src/main/linux/LiveSplitFixes");
const launchArgs = require("../src/main/game-delivery/launch-args");
const { resolveLibrary, ensureLayout } = require("../src/main/game-delivery/paths");
const { ensureAppImageDesktop, refreshMenuCaches, refreshKdeMenuCache, ensureManagedCli, managedCliPath } = require("../src/main/linux/DesktopIntegration");
const { parseListing } = require("../src/main/game-delivery/archive-entries");
const patchFormat = require("../src/main/game-delivery/patch-format");
const { NO_SYMLINKS, NO_CASE_SENSITIVITY, NO_EXEC_BIT, NO_SHELL_SCRIPTS, NO_GETUID, NO_POSIX_PATHS, NOT_LINUX } = require("./lib/selftest-caps");

function writeCoreFonts(prefix) {
  const directory = path.join(prefix, "drive_c/windows/Fonts");
  fs.mkdirSync(directory, { recursive: true });
  for (const name of ProtonRunner.CORE_FONT_FILES)
    fs.copyFileSync(path.join(__dirname, "../resources/livesplit-fonts/Carlito-Regular.ttf"), path.join(directory, name));
  fs.writeFileSync(path.join(prefix, "winetricks.log"), "arial\ntimes\ncorefonts\n");
}

function prefixRunner(t, spawnFn, options = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-prefix-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const testProton = path.join(root, "proton");
  fs.mkdirSync(path.join(testProton, "protonfixes"), { recursive: true });
  fs.writeFileSync(path.join(testProton, "protonfixes/winetricks"), "#!/bin/sh\n", { mode: 0o755 });
  const runner = new ProtonRunner({
    manager: { selected: () => ({ path: testProton, name: "Test Proton" }) },
    prefixWineservers: () => [],
    dataDir: root, resourcesPath: path.join(__dirname, "../resources"), spawnFn,
    pythonRuntime: { prepareEnvironment: async (env) => env },
    userNamespaces: { ensure: async () => {} }, ...options,
  });
  runner.umuPath = () => "/test/umu-run";
  const prefix = runner.prefixFor("ghosting-renewed");
  const game = path.join(root, "hl2.exe");
  fs.writeFileSync(game, "test executable");
  return { root, runner, prefix, game, marker: path.join(prefix, ".hlsr-corefonts-ready") };
}

function commandChild(callback) {
  const child = new EventEmitter();
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.kill = () => true;
  setImmediate(() => callback(child));
  return child;
}

function hostServerRunner(t, exitCode) {
  const calls = [];
  const fixture = prefixRunner(t, (command, args, options) => {
    calls.push({ command, args, options });
    const child = new EventEmitter();
    if (path.basename(command) === "wineserver") setImmediate(() => child.emit("exit", exitCode, null));
    else setImmediate(() => child.emit("spawn"));
    return child;
  });
  const proton = path.join(fixture.root, "proton");
  fs.mkdirSync(path.join(proton, "files/bin"), { recursive: true });
  fs.writeFileSync(path.join(proton, "files/bin/wineserver"), "#!/bin/sh\n", { mode: 0o755 });
  fixture.runner.manager = { selected: () => ({ path: proton, name: "Test Proton" }) };
  fs.mkdirSync(fixture.prefix, { recursive: true });
  fs.writeFileSync(path.join(fixture.prefix, "system.reg"), "WINE REGISTRY Version 2\n");
  return Object.assign(fixture, { calls, proton });
}

test("Proton: wineserver префикса поднимается на хосте до контейнера umu", async (t) => {
  const { runner, prefix, game, calls, proton } = hostServerRunner(t, 0);
  await runner.launch("ghosting-renewed", game, [], { skipFonts: true, env: { PROTON_NO_FSYNC: "1", PROTON_NO_NTSYNC: "1" } });
  assert.deepEqual(calls.map((call) => call.command), [path.join(proton, "files/bin/wineserver"), "/test/umu-run"]);
  const server = calls[0].options;
  assert.deepEqual(calls[0].args, [], "без -p: сервер уходит сам после последнего клиента");
  assert.equal(server.env.WINEPREFIX, prefix);
  assert.equal(server.env.WINEFSYNC, "0", "PROTON_NO_FSYNC выключает fsync и у сервера");
  assert.equal(server.env.PROTON_NO_NTSYNC, "1");
  assert.equal(server.env.LD_LIBRARY_PATH, undefined, "окружение AppImage серверу не передаётся");
  assert.equal(server.stdio, "ignore", "фоновый сервер не держит каналы лаунчера");

  calls.length = 0;
  await runner.launch("ghosting-renewed", game, [], { skipFonts: true });
  assert.equal(calls[0].options.env.WINEFSYNC, "1");
});

test("Proton: работающий wineserver и новый префикс не трогаются", async (t) => {
  const running = hostServerRunner(t, 2);
  const warnings = [];
  running.runner.logger = { info() {}, warn: (message) => warnings.push(message), error() {} };
  const launch = running.runner.environment("ghosting-renewed");
  assert.equal(await running.runner.ensureHostWineserver(launch, launch.env), "running");
  assert.deepEqual(warnings, []);

  fs.rmSync(path.join(running.prefix, "system.reg"));
  running.calls.length = 0;
  assert.equal(await running.runner.ensureHostWineserver(launch, launch.env), "no-prefix");
  assert.equal(running.calls.length, 0, "новый префикс создаёт Proton");

  const broken = hostServerRunner(t, 1);
  broken.runner.logger = { info() {}, warn: (message) => warnings.push(message), error() {} };
  await broken.runner.launch("ghosting-renewed", broken.game, [], { skipFonts: true });
  assert.equal(broken.calls.at(-1).command, "/test/umu-run", "без сервера на хосте запуск идёт как раньше");
  assert.match(warnings.join("\n"), /не запустился/);
});

test("Proton: wineserver другого Proton в префиксе останавливает запуск", async (t) => {
  const fixture = hostServerRunner(t, 2);
  const own = fs.statSync(path.join(fixture.proton, "files/bin/wineserver"));
  fixture.runner.prefixWineservers = () => [{ pid: 4242, dev: own.dev, ino: own.ino, exePath: null }];
  await fixture.runner.launch("ghosting-renewed", fixture.game, [], { skipFonts: true });
  assert.equal(fixture.calls.at(-1).command, "/test/umu-run", "сервер того же Proton не мешает");

  const other = fs.statSync(fixture.game);
  fixture.runner.prefixWineservers = () => [{ pid: 4242, dev: other.dev, ino: other.ino,
    exePath: "/home/u/.local/share/Steam/steamapps/common/Proton - Experimental/files/bin/wineserver" }];
  fixture.calls.length = 0;
  await assert.rejects(fixture.runner.launch("ghosting-renewed", fixture.game, [], { skipFonts: true }),
    (error) => error.code === "LAUNCH_FAILED" && /Proton Proton - Experimental/.test(error.message) &&
      /PID 4242/.test(error.message) && /LiveSplit/.test(error.message));
  assert.deepEqual(fixture.calls, [], "ни wineserver, ни umu не запускаются");
});

test("Proton: wineserver префикса находится по папке сокета и файлу программы", { skip: NO_GETUID || NO_SYMLINKS }, (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-wineserver-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const prefix = path.join(root, "pfx");
  fs.mkdirSync(prefix);
  const stat = fs.statSync(prefix);
  const serverDir = path.join(root, "tmp", `.wine-${process.getuid()}`, `server-${stat.dev.toString(16)}-${stat.ino.toString(16)}`);
  const otherDir = path.join(root, "tmp", `.wine-${process.getuid()}`, "server-1-2");
  fs.mkdirSync(serverDir, { recursive: true });
  fs.mkdirSync(otherDir, { recursive: true });
  const binary = path.join(root, "proton/files/bin/wineserver");
  fs.mkdirSync(path.dirname(binary), { recursive: true });
  fs.writeFileSync(binary, "server");
  const proc = path.join(root, "proc");
  for (const [pid, comm, cwd] of [["10", "wineserver", serverDir], ["11", "wineserver", otherDir], ["12", "hl2.exe", serverDir]]) {
    fs.mkdirSync(path.join(proc, pid), { recursive: true });
    fs.writeFileSync(path.join(proc, pid, "comm"), `${comm}\n`);
    fs.symlinkSync(cwd, path.join(proc, pid, "cwd"));
    fs.symlinkSync(binary, path.join(proc, pid, "exe"));
  }
  fs.mkdirSync(path.join(proc, "self"));
  const servers = ProtonRunner.prefixWineservers(prefix, proc, path.join(root, "tmp"));
  const own = fs.statSync(binary);
  assert.deepEqual(servers, [{ pid: 10, dev: own.dev, ino: own.ino, exePath: binary }]);
  assert.deepEqual(ProtonRunner.prefixWineservers(path.join(root, "missing"), proc, path.join(root, "tmp")), []);
});

test("Proton: сборка без protonfixes не зовёт winetricks и объясняет причину", async (t) => {
  const calls = [];
  const { runner, root, game } = prefixRunner(t, (command, args) => {
    calls.push(args);
    return commandChild((child) => child.emit("close", 0, null));
  });
  const valve = path.join(root, "Proton 11.0");
  fs.mkdirSync(valve, { recursive: true });
  runner.manager = { selected: () => ({ path: valve, name: "Proton 11.0" }) };
  await assert.rejects(runner.launch("ghosting-renewed", game),
    (error) => error.code === "LAUNCH_FAILED" && /Proton 11\.0 нет protonfixes\/winetricks/.test(error.message));
  assert.deepEqual(calls, []);
});

test("Proton: вывод запуска игры пишется в журнал с заголовком", async (t) => {
  let stdio = null;
  const { runner, root, game } = prefixRunner(t, (command, args, options) => {
    stdio = options.stdio;
    if (Array.isArray(stdio)) fs.writeSync(stdio[2], "err:module:import_dll Library X not found\n");
    const child = new EventEmitter();
    setImmediate(() => child.emit("spawn"));
    return child;
  });
  const logFile = path.join(root, "logs/proton-ghosting-renewed.log");
  fs.mkdirSync(path.dirname(logFile));
  fs.writeFileSync(logFile, "старый запуск\n");
  await runner.launch("ghosting-renewed", game, ["-novid"], { skipFonts: true, logFile });
  assert.equal(stdio[0], "ignore");
  assert.equal(stdio[1], stdio[2]);
  const text = fs.readFileSync(logFile, "utf8");
  assert.match(text, /^# \d{4}-\d\d-\d\dT.* Test Proton: .*hl2\.exe -novid\n/);
  assert.match(text, /Library X not found/);
  assert.doesNotMatch(text, /старый запуск/);

  await runner.launch("ghosting-renewed", game, [], { skipFonts: true });
  assert.equal(stdio, "ignore", "без файла журнала вывод не сохраняется");
});

test("Параметры Proton: переменные, обёртка и аргументы игры как в Steam", () => {
  assert.deepEqual(launchArgs.parseProtonOptions('PROTON_USE_WINED3D=1 DXVK_HUD="fps, frametimes" mangohud --dlsym %command% -novid'), {
    ok: true,
    env: { PROTON_USE_WINED3D: "1", DXVK_HUD: "fps, frametimes" },
    wrapper: ["mangohud", "--dlsym"],
    args: ["-novid"],
  });
  assert.deepEqual(launchArgs.parseProtonOptions("mangohud"), { ok: true, env: {}, wrapper: ["mangohud"], args: [] },
    "без %command% остаток строки — обёртка");
  assert.deepEqual(launchArgs.parseProtonOptions("PROTON_LOG=1 %command%"), { ok: true, env: { PROTON_LOG: "1" }, wrapper: [], args: [] });
  assert.deepEqual(launchArgs.parseProtonOptions(""), { ok: true, env: {}, wrapper: [], args: [] });
  assert.deepEqual(launchArgs.parseProtonOptions("WINEPREFIX=/tmp/x %command%").error, { code: "PROTECTED_ENV", param: "WINEPREFIX" });
  assert.deepEqual(launchArgs.parseProtonOptions("STEAM_COMPAT_DATA_PATH=/x").error, { code: "PROTECTED_ENV", param: "STEAM_COMPAT_DATA_PATH" });
  assert.deepEqual(launchArgs.parseProtonOptions("-novid").error, { code: "WRAPPER_INVALID", param: "-novid" });
  assert.deepEqual(launchArgs.parseProtonOptions("%command% -game cstrike").error, { code: "FORBIDDEN_PARAM", param: "-game" });
  assert.equal(launchArgs.parseProtonOptions("%command% %command%").ok, false);
  assert.equal(launchArgs.parseProtonOptions('mangohud "%command%').error.code, "UNBALANCED_QUOTES");

  assert.equal(launchArgs.normalizeSettings({ protonOptions: "  mangohud %command%  " }).settings.protonOptions, "mangohud %command%");
  assert.deepEqual(launchArgs.normalizeSettings({ protonOptions: "GAMEID=1" }).error,
    { field: "protonOptions", code: "PROTECTED_ENV", param: "GAMEID" }, "ошибка указывает на поле Proton");
  const sanitized = launchArgs.sanitizeSettings({ launchArgs: "-high", protonOptions: "WINEPREFIX=/x" });
  assert.equal(sanitized.settings.protonOptions, "", "испорченная строка из файла не доходит до запуска");
  assert.equal(sanitized.settings.launchArgs, "-high");
  assert.equal(sanitized.dropped.field, "protonOptions");
  assert.equal(launchArgs.defaultSettings().protonOptions, "");
});

test("Proton: обёртка из параметров запускает umu-run, переменные идут в окружение", async (t) => {
  const { root, runner, game, calls } = hostServerRunner(t, 2);
  const bin = path.join(root, "bin");
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(bin, "mangohud"), "#!/bin/sh\n", { mode: 0o755 });
  const savedPath = process.env.PATH;
  process.env.PATH = `${bin}${path.delimiter}${savedPath}`;
  t.after(() => { process.env.PATH = savedPath; });

  await runner.launch("ghosting-renewed", game, ["-novid"], {
    skipFonts: true, wrapper: ["mangohud", "--dlsym"], env: { PROTON_USE_WINED3D: "1" },
  });
  const spawned = calls.at(-1);
  assert.equal(spawned.command, path.join(bin, "mangohud"));
  assert.deepEqual(spawned.args, ["--dlsym", "/test/umu-run", game, "-novid"]);
  assert.equal(spawned.options.env.PROTON_USE_WINED3D, "1");

  await assert.rejects(runner.launch("ghosting-renewed", game, [], { skipFonts: true, wrapper: ["no-such-wrapper-hlsr"] }),
    (error) => error.code === "PROTON_WRAPPER_MISSING" && /no-such-wrapper-hlsr/.test(error.message));
  await assert.rejects(runner.launch("ghosting-renewed", game, [], { skipFonts: true, wrapper: ["bin/mangohud"] }),
    (error) => error.code === "PROTON_WRAPPER_MISSING", "относительный путь не ищется");
});

test("Linux 7za: папка payload с атрибутом Dn не считается файлом вне патча", () => {
  const listing = parseListing([
    "Path = example.7z",
    "Type = 7z",
    "",
    "----------",
    "Path = payload",
    "Size = 0",
    "Attributes = Dn",
    "",
    "Path = payload/ghostingmod",
    "Size = 0",
    "Attributes = Dn",
    "",
    "Path = payload/ghostingmod/file.txt",
    "Size = 4",
    "Attributes = A",
    "",
  ].join("\n"));
  assert.equal(listing.entries[0].isDirectory, true);
  assert.equal(listing.entries[1].isDirectory, true);
  assert.equal(patchFormat.validatePayloadEntries(
    { files: [{ path: "ghostingmod/file.txt" }] }, listing.entries), true);
  assert.throws(() => patchFormat.validatePayloadEntries(
    { files: [{ path: "ghostingmod/file.txt" }] },
    listing.entries.concat({ path: "outside.txt", isDirectory: false })));
});

test("Proton: обнаружение, выбор и один префикс для игры и LiveSplit", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-linux-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const proton = path.join(root, "protons", "Proton-Test");
  const umu = path.join(root, "resources", "umu", "umu-run");
  const game = path.join(root, "game", "hl2.exe");
  const timer = path.join(root, "tools", "LiveSplit.exe");
  for (const file of [path.join(proton, "proton"), path.join(proton, "protonfixes/winetricks"), umu, game, timer]) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "test");
  }
  fs.chmodSync(umu, 0o755);
  fs.chmodSync(path.join(proton, "proton"), 0o755);
  fs.cpSync(path.join(__dirname, "../resources/livesplit-fonts"),
    path.join(root, "resources/livesplit-fonts"), { recursive: true });
  const manager = new ProtonManager({ dataDir: root, home: root });
  assert.ok(manager.discover().some((entry) => entry.path === proton));
  manager.select(proton);
  assert.equal(manager.selected().path, proton);
  const custom = path.join(root, "other", "Proton-Custom");
  fs.mkdirSync(custom, { recursive: true });
  fs.writeFileSync(path.join(custom, "proton"), "test");
  fs.chmodSync(path.join(custom, "proton"), 0o755);
  assert.equal(manager.addCustom(custom).path, custom);
  assert.ok(manager.discover().some((entry) => entry.path === custom));
  assert.equal(manager.selected().path, proton, "добавление не меняет выбор до сохранения");

  const calls = [];
  const spawnFn = (command, args, options) => {
    calls.push({ command, args, options });
    const child = new EventEmitter();
    child.pid = 12345;
    child.unref = () => {};
    setImmediate(() => {
      child.emit("spawn");
      if (args[0] === "winetricks") writeCoreFonts(options.env.WINEPREFIX);
      if (["winetricks", "regedit.exe", "wineboot.exe"].includes(args[0]))
        child.emit("close", 0, null);
    });
    return child;
  };
  const runner = new ProtonRunner({ manager, dataDir: root, resourcesPath: path.join(root, "resources"), spawnFn,
    pythonRuntime: { prepareEnvironment: async (env) => env }, userNamespaces: { ensure: async () => {} } });
  await runner.launch("ghosting-renewed", game);
  await runner.launch("ghosting-renewed", timer, [], { fontProfile: "livesplit" });
  assert.equal(calls.length, 4, "corefonts и профиль LiveSplit устанавливаются один раз");
  assert.deepEqual(calls[0].args, ["winetricks", "corefonts"]);
  assert.equal(calls[1].args[0], game);
  assert.equal(calls[2].args[0], "regedit.exe");
  assert.equal(calls[3].args[0], timer);
  for (const call of calls) {
    assert.equal(call.command, umu);
    assert.equal(call.options.env.WINEPREFIX, runner.prefixFor("ghosting-renewed"));
    assert.equal(call.options.env.PROTONPATH, proton);
    assert.equal(call.options.env.PROTON_VERB, "run", "игра и LiveSplit должны запускаться одновременно");
    assert.equal(call.options.env.PROTON_LOCAL_SHADER_CACHE, "1");
  }
  assert.equal(fs.existsSync(path.join(runner.prefixFor("ghosting-renewed"),
    ProtonRunner.LIVESPLIT_FONT_MARKER)), true);
  for (const font of ProtonRunner.LIVESPLIT_FONTS)
    assert.equal(fs.existsSync(path.join(runner.prefixFor("ghosting-renewed"),
      "drive_c/windows/Fonts", font.target)), true);
  await runner.launch("ghosting-renewed", timer, [], { fontProfile: "livesplit" });
  assert.equal(calls.length, 5, "повторный запуск не переустанавливает шрифты");
  await runner.launch("ghosting-renewed", game, [], { excludeCpu0: true });
  const affinityCall = calls[5];
  if (ProtonRunner.allowedCpuListWithoutZero() && ProtonRunner.executableInPath("taskset")) {
    assert.equal(affinityCall.command, ProtonRunner.executableInPath("taskset"));
    assert.equal(affinityCall.args[0], "-c");
    assert.equal(affinityCall.args[2], umu);
  }
  assert.equal(affinityCall.options.env.WINEPREFIX, runner.prefixFor("ghosting-renewed"));
  assert.equal(affinityCall.options.env.PROTON_VERB, "run");
});

test("Proton: существующие corefonts без маркера HLSR не запускают повторный winetricks", async (t) => {
  const calls = [];
  const { runner, prefix, marker, game } = prefixRunner(t, (command, args) => {
    calls.push(args);
    return commandChild((child) => child.emit("spawn"));
  });
  writeCoreFonts(prefix);
  assert.equal(fs.existsSync(marker), false);
  await runner.launch("ghosting-renewed", game);
  assert.deepEqual(calls, [[game]]);
  assert.equal(fs.existsSync(marker), true);
  assert.equal(ProtonRunner.coreFontsStatus(prefix).ready, true);
});

test("Proton: параллельная подготовка шрифтов разделяет одну установку", async (t) => {
  let count = 0;
  const { runner, prefix } = prefixRunner(t, () => commandChild((child) => {
    count++;
    writeCoreFonts(prefix);
    child.emit("close", 0, null);
  }));
  await Promise.all([runner.ensureCoreFonts("ghosting-renewed"), runner.ensureCoreFonts("ghosting-renewed")]);
  assert.equal(count, 1);
  assert.equal(runner.fontsPending.size, 0);
});

test("Proton: холодный префикс с exit 0 без corefonts повторяется один раз", async (t) => {
  let attempts = 0;
  const { runner, prefix, marker } = prefixRunner(t, () => commandChild((child) => {
    attempts++;
    if (attempts === 2) writeCoreFonts(prefix);
    child.emit("close", 0, null);
  }));
  await Promise.all([runner.ensureCoreFonts("ghosting-renewed"), runner.ensureCoreFonts("ghosting-renewed")]);
  assert.equal(attempts, 2);
  assert.equal(fs.existsSync(marker), true);
  await runner.ensureCoreFonts("ghosting-renewed");
  assert.equal(attempts, 2);
});

test("Proton: exit 0 без шрифтов сохраняет диагностику и не оставляет маркер", async (t) => {
  const { runner, marker } = prefixRunner(t, () => commandChild((child) => {
    child.stderr.emit("data", "HLSR_TOKEN=secret\nRuntime initialization incomplete\n");
    child.emit("close", 0, null);
  }));
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), (error) => {
    assert.ok(error.message.includes("Runtime initialization incomplete"));
    assert.ok(!error.message.includes("secret"));
    return true;
  });
  assert.equal(fs.existsSync(marker), false);
});

test("Python: рабочий системный интерпретатор переиспользуется и изолируется от user site", { skip: NO_POSIX_PATHS }, async (t) => {
  const PythonRuntime = require("../src/main/linux/PythonRuntime");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-python-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const runtime = new PythonRuntime({ dataDir: root, probe: async (file) => file === "/usable/bin/python3" });
  runtime.install = async () => assert.fail("system Python must avoid downloads");
  const env = await runtime.prepareEnvironment({ PATH: "/old/bin:/usable/bin", DISPLAY: ":0" });
  assert.equal(env.PATH, "/usable/bin:/old/bin:/usable/bin");
  assert.equal(env.PYTHONNOUSERSITE, "1");
  assert.equal(env.DISPLAY, ":0");
});

test("Python: отсутствие интерпретатора объединяет параллельную загрузку и допускает повтор ошибки", async (t) => {
  const PythonRuntime = require("../src/main/linux/PythonRuntime");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-python-download-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  let installed = false, fail = true, downloads = 0;
  const runtime = new PythonRuntime({ dataDir: root, probe: async (file) => installed && file.startsWith(root) });
  runtime.install = async () => { downloads++; if (fail) throw new Error("offline"); installed = true; };
  const failed = await Promise.allSettled([runtime.ensure({ PATH: "/empty" }), runtime.ensure({ PATH: "/empty" })]);
  assert.ok(failed.every((result) => result.status === "rejected"));
  assert.equal(downloads, 1);
  assert.equal(runtime.pending, null);
  fail = false;
  const ready = await Promise.all([runtime.ensure({ PATH: "/empty" }), runtime.ensure({ PATH: "/empty" })]);
  assert.equal(ready[0], ready[1]);
  assert.equal(downloads, 2);
  assert.ok(ready[0].endsWith(path.join("/bin/python3")));
});

test("linux-unpacked: start работает из пути с пробелами и передаёт аргументы без AppImage env", { skip: NO_SHELL_SCRIPTS }, (t) => {
  const { spawnSync } = require("child_process");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr unpacked "));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "hlsr"), '#!/bin/sh\nprintf "%s\\n" "$@" "APPDIR=${APPDIR-unset}"\n', { mode: 0o755 });
  require("./after-pack-linux")({ electronPlatformName: "linux", appOutDir: root });
  const result = spawnSync(path.join(root, "start"), ["--select-game=ghosting-renewed", "two words"], {
    encoding: "utf8", env: { ...process.env, APPDIR: "/fake/appimage" },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.stdout.trim().split("\n"), ["--no-sandbox", "--select-game=ghosting-renewed", "two words", "APPDIR=unset"]);
});

test("AppArmor: подтверждение запрашивается один раз только после отказа bwrap", { skip: NO_GETUID }, async (t) => {
  const UserNamespaces = require("../src/main/linux/UserNamespaces");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-userns-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bwrap = path.join(root, "umu/steamrt4/pressure-vessel/libexec/steam-runtime-tools-0/srt-bwrap");
  fs.mkdirSync(path.dirname(bwrap), { recursive: true });
  fs.writeFileSync(bwrap, "test");
  let granted = false, approvals = 0;
  const support = new UserNamespaces({ resourcesPath: "/app/resources", isRestricted: () => true,
    execute: async (file, args) => {
      if (file === "/usr/bin/pkexec") {
        approvals++;
        assert.deepEqual(args, ["/bin/bash", "/app/resources/linux/install-userns-profile.sh", String(process.getuid()), path.join(root, "umu")]);
        granted = true;
      } else { assert.equal(file, bwrap); if (!granted) throw new Error("Permission denied"); }
    },
  });
  await Promise.all([support.ensure({ XDG_DATA_HOME: root }), support.ensure({ XDG_DATA_HOME: root })]);
  await support.ensure({ XDG_DATA_HOME: root });
  assert.equal(approvals, 1);
});

test("AppArmor: отсутствие ограничения не запускает команды и пути umu учитывают XDG", async () => {
  const UserNamespaces = require("../src/main/linux/UserNamespaces");
  const support = new UserNamespaces({ isRestricted: () => false, execute: async () => assert.fail("no commands needed") });
  await support.ensure({});
  // path.join даёт на Linux ровно "/data/umu", на Windows — те же пути с «\».
  assert.equal(UserNamespaces.umuRoot({ XDG_DATA_HOME: "/data" }), path.join("/data/umu"));
  assert.equal(UserNamespaces.umuRoot({ UMU_FOLDERS_PATH: "/custom", XDG_DATA_HOME: "/data" }), path.join("/custom/umu"));
  assert.equal(UserNamespaces.umuRoot({ container: "flatpak", HOST_XDG_DATA_HOME: "/host" }), path.join("/host/umu"));
});

test("Proton: маркер или запись winetricks без шрифтов не скрывает повреждённый префикс", async (t) => {
  let calls = 0;
  const { runner, prefix, marker } = prefixRunner(t, () => commandChild((child) => {
    calls++;
    child.emit("close", 0, null);
  }));
  fs.mkdirSync(prefix, { recursive: true });
  fs.writeFileSync(marker, "old launcher marker");
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), /без необходимых шрифтов/);
  assert.equal(calls, 2, "a stale marker must not suppress the bounded cold-prefix retry");
  fs.writeFileSync(path.join(prefix, "winetricks.log"), "corefonts\n");
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), /отмечены установленными.*arial.ttf/);
  assert.equal(calls, 2, "umu refuses already logged verbs; report the damaged files directly");
});

test("Proton: ошибки подготовки сохраняют код и ограниченную очищенную диагностику", async (t) => {
  const { runner, marker } = prefixRunner(t, (command, args, options) => {
    assert.deepEqual(options.stdio, ["ignore", "pipe", "pipe"]);
    assert.equal(options.detached, true);
    return commandChild((child) => {
      child.stdout.emit("data", "x".repeat(40000) + "\ncorefonts download\n");
      child.stderr.emit("data", "HLSR_PRESENCE_TOKEN=do-not-log-me\nCannot find cabextract\n");
      child.emit("close", 1, null);
    });
  });
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), (error) => {
    assert.equal(error.code, "PROTON_PREFIX_COMMAND_FAILED");
    assert.equal(error.exitCode, 1);
    assert.ok(error.message.includes("umu-run winetricks corefonts"));
    assert.ok(error.message.includes("Cannot find cabextract"));
    assert.ok(error.message.includes("corefonts download"));
    assert.ok(!error.message.includes("do-not-log-me"));
    assert.ok(error.stdout.length <= 8192);
    assert.ok(error.stderr.length <= 8192);
    return true;
  });
  assert.equal(fs.existsSync(marker), false);
});

test("Proton: зависшая подготовка останавливается по таймауту и допускает повтор", async (t) => {
  let retry = false;
  const signals = [];
  const { runner, prefix, marker } = prefixRunner(t, () => {
    const child = commandChild((process) => {
      if (!retry) { process.stderr.emit("data", "download stalled"); return; }
      writeCoreFonts(prefix);
      process.emit("close", 0, null);
    });
    child.kill = (signal) => { signals.push(signal); return true; };
    return child;
  }, { prefixCommandTimeoutMs: 20, prefixCommandKillGraceMs: 10 });
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), (error) => {
    assert.equal(error.code, "PROTON_PREFIX_COMMAND_TIMEOUT");
    assert.ok(error.message.includes("download stalled"));
    return true;
  });
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
  assert.equal(runner.fontsPending.size, 0);
  assert.equal(fs.existsSync(marker), false);
  retry = true;
  await runner.ensureCoreFonts("ghosting-renewed");
  assert.equal(fs.existsSync(marker), true);
});

test("Proton: ошибка spawn ENOENT остаётся ошибкой запуска", async (t) => {
  const { runner, marker } = prefixRunner(t, () => commandChild((child) => {
    const error = new Error("spawn /test/umu-run ENOENT");
    error.code = "ENOENT";
    child.emit("error", error);
  }));
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), (error) => error.code === "ENOENT" && /ENOENT/.test(error.message));
  assert.equal(fs.existsSync(marker), false);
});

test("Proton: установленный параллельно corefonts принимается только для явного already installed", async (t) => {
  let unrelatedFailure = false;
  const { runner, prefix, marker } = prefixRunner(t, () => commandChild((child) => {
    writeCoreFonts(prefix);
    child.stderr.emit("data", unrelatedFailure ? "runtime initialization failed" : `winetricks verb 'corefonts' is already installed in '${prefix}'`);
    child.emit("close", 1, null);
  }));
  await runner.ensureCoreFonts("ghosting-renewed");
  assert.equal(fs.existsSync(marker), true);
  fs.rmSync(prefix, { recursive: true });
  unrelatedFailure = true;
  await assert.rejects(runner.ensureCoreFonts("ghosting-renewed"), /runtime initialization failed/);
  assert.equal(fs.existsSync(marker), false);
});

test("Proton: отмена предпросмотра во время общей установки не запускает позднюю игру", async (t) => {
  const calls = [];
  const abort = new AbortController();
  const { runner, prefix, game } = prefixRunner(t, (command, args) => commandChild((child) => {
    calls.push(args);
    writeCoreFonts(prefix);
    abort.abort();
    child.emit("close", 0, null);
  }));
  await assert.rejects(runner.launch("ghosting-renewed", game, [], { signal: abort.signal }), (error) => error.code === "ABORT_ERR");
  assert.deepEqual(calls, [["winetricks", "corefonts"]]);
  assert.equal(ProtonRunner.coreFontsStatus(prefix).ready, true, "shared font preparation must finish");
  await assert.rejects(runner.launch("ghosting-renewed", game, [], { skipFonts: true, signal: abort.signal }), (error) => error.code === "ABORT_ERR");
  assert.equal(calls.length, 1);
});

test("LiveSplit на Windows: запущенный отслеживается и закрывается через taskkill", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-tool-win-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = ensureLayout(resolveLibrary(path.join(root, "library")));
  const toolDir = path.join(paths.tools, "livesplit");
  fs.mkdirSync(toolDir, { recursive: true });
  fs.writeFileSync(path.join(toolDir, "LiveSplit.exe"), "test");
  fs.writeFileSync(path.join(toolDir, "installed.json"), JSON.stringify({
    toolId: "livesplit", version: "1.0.0", executable: "LiveSplit.exe", installPath: toolDir,
  }));

  const children = [];
  const calls = [];
  const spawnFn = (exe, args, options) => {
    assert.equal(options.detached, true, "LiveSplit должен переживать закрытие лаунчера");
    const child = new EventEmitter();
    child.pid = 5000 + children.length;
    child.exitCode = null;
    child.unref = () => {};
    children.push(child);
    return child;
  };
  // Мягкое закрытие: окно спросило про сплиты и не закрылось — дальше /F.
  const stopper = {
    graceful: async (pid) => { calls.push(["graceful", pid]); return { ok: true, code: 0, output: "" }; },
    force: async (pid) => {
      calls.push(["force", pid]);
      const child = children.find((item) => item.pid === pid);
      setImmediate(() => { child.exitCode = 1; child.emit("exit", 1, null); });
      return { ok: true, code: 0, output: "" };
    },
  };
  const service = new ToolService({
    paths, platform: "win32", spawnFn, stopper,
    registry: { livesplit: { version: "1.0.0", executable: "LiveSplit.exe" } },
    signalFn: () => { throw new Error("POSIX-сигнал на Windows не используется"); },
  });
  const realSetTimeout = global.setTimeout;
  global.setTimeout = (fn, ms, ...rest) => realSetTimeout(fn, Math.min(ms, 20), ...rest);
  t.after(() => { global.setTimeout = realSetTimeout; });

  assert.equal((await service.launch("livesplit")).status, "started");
  assert.equal(service.status("livesplit").running, true, "кнопка должна стать «Закрыть»");
  assert.equal((await service.launch("livesplit")).status, "already-running", "второй экземпляр не запускается");
  assert.equal(children.length, 1);

  assert.equal((await service.stop("livesplit")).status, "timeout");
  assert.equal(service.status("livesplit").running, true);
  assert.equal((await service.stop("livesplit", { force: true })).status, "stopped");
  assert.deepEqual(calls, [["graceful", 5000], ["force", 5000]]);
  assert.equal(service.status("livesplit").running, false);

  // Закрытый самим пользователем LiveSplit снова запускается из лаунчера.
  assert.equal((await service.launch("livesplit")).status, "started");
  children[1].exitCode = 0;
  children[1].emit("exit", 0, null);
  assert.equal(service.status("livesplit").running, false);
});

test("LiveSplit: повторный запуск и закрытие в отдельной группе процессов", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-tool-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = ensureLayout(resolveLibrary(path.join(root, "library")));
  const toolDir = path.join(paths.tools, "livesplit");
  fs.mkdirSync(toolDir, { recursive: true });
  fs.writeFileSync(path.join(toolDir, "LiveSplit.exe"), "test");
  fs.writeFileSync(path.join(toolDir, "installed.json"), JSON.stringify({
    toolId: "livesplit", version: "1.0.0", executable: "LiveSplit.exe", installPath: toolDir,
  }));

  const child = new EventEmitter();
  child.pid = 4242;
  child.unref = () => {};
  const signals = [];
  const states = [];
  const service = new ToolService({
    // Ветка Linux явно: иначе на Windows ToolService запускает LiveSplit.exe напрямую.
    paths, platform: "linux",
    hostEnvironment: { DISPLAY: ":0" },
    registry: { livesplit: { version: "1.0.0", executable: "LiveSplit.exe" } },
    protonRunner: { launch: async (gameId, exe, args, options) => {
      assert.equal(gameId, "ghosting-renewed");
      assert.equal(exe, path.join(toolDir, "LiveSplit.exe"));
      assert.deepEqual(options.env, { PROTON_USE_X11_EXCLUSIVE: "LiveSplit.exe" });
      assert.equal(options.fontProfile, "livesplit");
      return { child, prefix: path.join(root, "prefix") };
    } },
    signalFn: (pid, signal) => {
      signals.push({ pid, signal });
      setImmediate(() => child.emit("exit", 0, signal));
    },
  });
  service.on("state", (toolId) => states.push({ toolId, ...service.status(toolId) }));
  assert.equal((await service.launch("livesplit", { gameId: "ghosting-renewed" })).status, "started");
  assert.equal(service.status("livesplit").running, true);
  assert.equal((await service.launch("livesplit")).status, "already-running");
  assert.throws(() => service.remove("livesplit"), { code: "ALREADY_RUNNING" });
  assert.equal((await service.stop("livesplit")).status, "stopped");
  assert.deepEqual(signals, [{ pid: -4242, signal: "SIGTERM" }]);
  assert.equal(service.status("livesplit").running, false);
  assert.deepEqual(states.map((state) => [state.running, state.stopping]),
    [[true, false], [true, true], [false, false]]);
});

test("LiveSplit: закрытый своим крестиком при живой сессии umu больше не считается открытым", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-tool-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = ensureLayout(resolveLibrary(path.join(root, "library")));
  const toolDir = path.join(paths.tools, "livesplit");
  fs.mkdirSync(toolDir, { recursive: true });
  fs.writeFileSync(path.join(toolDir, "LiveSplit.exe"), "test");
  fs.writeFileSync(path.join(toolDir, "installed.json"), JSON.stringify({
    toolId: "livesplit", version: "1.0.0", executable: "LiveSplit.exe", installPath: toolDir,
  }));
  // Сессия umu не выходит: в префиксе ещё работает игра.
  const child = new EventEmitter();
  child.pid = 5151;
  child.unref = () => {};
  const signals = [];
  let alive = [false, true, true, false, false];
  const service = new ToolService({
    paths, platform: "linux", watchIntervalMs: 5,
    registry: { livesplit: { version: "1.0.0", executable: "LiveSplit.exe" } },
    liveSplitFixes: () => [],
    processRunning: (name) => {
      assert.equal(name, "LiveSplit.exe");
      return alive.length ? alive.shift() : false;
    },
    protonRunner: { launch: async () => ({ child, prefix: path.join(root, "prefix") }) },
    signalFn: (pid, signal) => signals.push({ pid, signal }),
  });
  const closed = new Promise((resolve) => service.on("state", () => {
    if (!service.status("livesplit").running) resolve();
  }));
  assert.equal((await service.launch("livesplit", { gameId: "ghosting-renewed" })).status, "started");
  assert.equal(service.status("livesplit").running, true, "пока exe не появился, LiveSplit считается открытым");
  await closed;
  assert.equal(alive.length, 0, "закрытие засчитывается только после двух пропусков подряд");
  assert.deepEqual(signals, [{ pid: -5151, signal: "SIGTERM" }], "сессия umu закрывается вместе с LiveSplit");
  assert.equal(service.runningInfo("livesplit"), null);
  // Снова запускается, а не отвечает «уже запущен».
  alive = [true];
  assert.equal((await service.launch("livesplit", { gameId: "ghosting-renewed" })).status, "started");
});

function liveSplitFixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-livesplit-fix-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const tool = path.join(root, "livesplit");
  const resources = path.join(root, "resources");
  fs.mkdirSync(path.join(tool, "Components"), { recursive: true });
  fs.mkdirSync(path.join(resources, LiveSplitFixes.FIXES_DIRECTORY), { recursive: true });
  const sha = (text) => require("crypto").createHash("sha256").update(text).digest("hex");
  const fixes = [{ target: "Components/Split.dll", source: "Split.dll", original: sha("исходный"), fixed: sha("исправленный") }];
  fs.writeFileSync(path.join(resources, LiveSplitFixes.FIXES_DIRECTORY, "Split.dll"), "исправленный");
  return { tool, resources, fixes, target: path.join(tool, "Components/Split.dll") };
}

test("LiveSplit: исправленный компонент заменяет только ту сборку, из которой сделан", (t) => {
  const { tool, resources, fixes, target } = liveSplitFixture(t);
  const apply = () => LiveSplitFixes.applyLiveSplitFixes(tool, { resourcesPath: resources, fixes });

  assert.deepEqual(apply(), [], "без компонента заменять нечего");
  fs.writeFileSync(target, "исходный");
  assert.deepEqual(apply(), ["Components/Split.dll"]);
  assert.equal(fs.readFileSync(target, "utf8"), "исправленный");
  assert.deepEqual(apply(), [], "исправленный файл повторно не копируется");
  assert.deepEqual(fs.readdirSync(path.dirname(target)), ["Split.dll"], "временный файл не остаётся");

  fs.writeFileSync(target, "обновлён пользователем");
  assert.deepEqual(apply(), []);
  assert.equal(fs.readFileSync(target, "utf8"), "обновлён пользователем");

  fs.writeFileSync(target, "исходный");
  fs.writeFileSync(path.join(resources, LiveSplitFixes.FIXES_DIRECTORY, "Split.dll"), "битый");
  assert.throws(apply, /повреждено/);
  assert.equal(fs.readFileSync(target, "utf8"), "исходный", "битое исправление не попадает в LiveSplit");
});

test("LiveSplit: исправленный SourceSplit в ресурсах совпадает с таблицей", () => {
  const crypto = require("crypto");
  for (const fix of LiveSplitFixes.LIVESPLIT_FIXES) {
    const file = path.join(__dirname, "../resources", LiveSplitFixes.FIXES_DIRECTORY, fix.source);
    assert.equal(crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex"), fix.fixed, fix.source);
    assert.notEqual(fix.original, fix.fixed);
  }
});

test("LiveSplit: исправления применяются перед запуском, а их ошибка запуск не останавливает", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-tool-fix-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = ensureLayout(resolveLibrary(path.join(root, "library")));
  const toolDir = path.join(paths.tools, "livesplit");
  fs.mkdirSync(toolDir, { recursive: true });
  fs.writeFileSync(path.join(toolDir, "LiveSplit.exe"), "test");
  fs.writeFileSync(path.join(toolDir, "installed.json"), JSON.stringify({
    toolId: "livesplit", version: "1.0.0", executable: "LiveSplit.exe", installPath: toolDir,
  }));
  const calls = [];
  const warnings = [];
  let fail = false;
  const service = new ToolService({
    paths, platform: "linux", resourcesPath: "/test/resources",
    logger: { info() {}, warn: (message) => warnings.push(message), error() {} },
    registry: { livesplit: { version: "1.0.0", executable: "LiveSplit.exe" } },
    liveSplitFixes: (dir, options) => {
      calls.push(["fix", dir, options.resourcesPath]);
      if (fail) throw new Error("нет ресурса");
      return [];
    },
    protonRunner: { launch: async () => {
      calls.push(["launch"]);
      const child = new EventEmitter();
      child.pid = 4343;
      child.unref = () => {};
      return { child, prefix: path.join(root, "prefix") };
    } },
  });

  assert.equal((await service.launch("livesplit")).status, "started");
  assert.deepEqual(calls, [["fix", toolDir, "/test/resources"], ["launch"]]);
  service.running.get("livesplit").child.emit("exit", 0, null);

  fail = true;
  assert.equal((await service.launch("livesplit")).status, "started");
  assert.equal(calls.at(-1)[0], "launch");
  assert.match(warnings.join("\n"), /нет ресурса/);
});

test("LiveSplit: XWayland override применяется только к таймеру и только при доступном X11", () => {
  assert.deepEqual(ToolService.toolLaunchEnvironment("livesplit", { DISPLAY: ":0" }), {
    PROTON_USE_X11_EXCLUSIVE: "LiveSplit.exe",
  });
  // Режим дисплея задаёт префикс, а не LiveSplit.
  assert.equal(ToolService.toolLaunchEnvironment("livesplit", {
    DISPLAY: ":0", WAYLAND_DISPLAY: "wayland-0",
  }).PROTON_ENABLE_WAYLAND, undefined);
  assert.equal(ToolService.toolLaunchEnvironment("other-tool", { DISPLAY: ":0" }), null);
  assert.equal(ToolService.toolLaunchEnvironment("livesplit", {}), null);
});

test("AppImage: ярлык и значок создаются без перезаписи чужого ярлыка", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-icon-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const appImage = path.join(root, "HLSR.AppImage");
  const iconSource = path.join(root, "hlsr.png");
  const dataHome = path.join(root, "data");
  fs.writeFileSync(appImage, "appimage");
  fs.writeFileSync(iconSource, "icon");
  assert.equal(ensureAppImageDesktop({ appImage, iconSource, dataHome }), true);
  const desktopPath = path.join(dataHome, "applications", "hlsr.desktop");
  const desktop = fs.readFileSync(desktopPath, "utf8");
  assert.match(desktop, /Icon=hlsr-appimage/);
  assert.match(desktop, /StartupWMClass=hlsr/);
  assert.match(desktop, /Categories=Game;/);
  assert.match(desktop, /X-HLSR-Managed=true/);
  assert.equal(ensureAppImageDesktop({ appImage, iconSource, dataHome, refresh: false }), true);
  assert.equal(fs.readFileSync(path.join(dataHome, "icons", "hicolor", "256x256", "apps", "hlsr-appimage.png"), "utf8"), "icon");
  fs.writeFileSync(desktopPath, "[Desktop Entry]\nName=Custom HLSR\n");
  assert.equal(ensureAppImageDesktop({ appImage, iconSource, dataHome }), false);
  assert.match(fs.readFileSync(desktopPath, "utf8"), /Name=Custom HLSR/);
  assert.equal(typeof refreshMenuCaches, "function");
});

test("KDE: после удаления меню пересобирается системными утилитами", { skip: NO_SHELL_SCRIPTS }, (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-kde-menu-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bin = path.join(root, "bin");
  const log = path.join(root, "calls");
  const dataHome = path.join(root, "data");
  fs.mkdirSync(bin);
  for (const command of ["kbuildsycoca6", "kbuildsycoca5"]) {
    const file = path.join(bin, command);
    fs.writeFileSync(file, `#!/bin/sh\nprintf '%s|%s|%s|%s\\n' '${command}' "$1" "$XDG_DATA_HOME" "$LD_LIBRARY_PATH" >> "$HLSR_TEST_KDE_LOG"\n`);
    fs.chmodSync(file, 0o755);
  }
  const keys = ["PATH", "APPDIR", "LD_LIBRARY_PATH", "HLSR_TEST_KDE_LOG"];
  const previous = Object.fromEntries(keys.map((key) => [key, process.env[key]]));
  Object.assign(process.env, { PATH: bin, APPDIR: "/appimage", LD_LIBRARY_PATH: "/appimage/lib", HLSR_TEST_KDE_LOG: log });
  try { refreshKdeMenuCache(dataHome); }
  finally {
    for (const key of keys) {
      if (previous[key] === undefined) delete process.env[key];
      else process.env[key] = previous[key];
    }
  }
  assert.deepEqual(fs.readFileSync(log, "utf8").trim().split("\n"), [
    `kbuildsycoca6|--noincremental|${dataHome}|`,
    `kbuildsycoca5|--noincremental|${dataHome}|`,
  ]);
});

test("CLI: команда hlsr запускает стабильный start из пути с пробелами и передаёт аргументы", { skip: NO_EXEC_BIT || NO_SHELL_SCRIPTS }, (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cli-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const home = path.join(root, "home");
  const installRoot = path.join(root, "managed install's $literal");
  fs.mkdirSync(installRoot, { recursive: true });
  fs.writeFileSync(path.join(installRoot, ".hlsr-install.json"), '{"schema":1}\n');
  fs.writeFileSync(path.join(installRoot, "start"), '#!/bin/sh\nROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 1\nprintf "%s\\n" "$ROOT" "$@"\n', { mode: 0o755 });
  assert.equal(ensureManagedCli({ installRoot, home }), true);
  const file = path.join(home, ".local/bin/hlsr");
  assert.equal(managedCliPath(installRoot, { home }), file);
  assert.equal(fs.statSync(file).mode & 0o777, 0o755);
  const initial = fs.readFileSync(file, "utf8");
  assert.equal(ensureManagedCli({ installRoot, home }), true);
  assert.equal(fs.readFileSync(file, "utf8"), initial);
  const args = ["--select-game=ghosting-renewed", "two words", "literal; $(echo NOT_EXECUTED)"];
  const launched = require("child_process").spawnSync(file, args, { cwd: root, encoding: "utf8" });
  assert.equal(launched.status, 0, launched.stderr);
  assert.deepEqual(launched.stdout.trim().split("\n"), [installRoot, ...args]);
});

test("CLI: установка и удаление сохраняют чужой файл или висячую ссылку hlsr", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cli-foreign-"));
  const home = path.join(root, "home");
  const installRoot = path.join(root, "install");
  const cli = path.join(home, ".local/bin/hlsr");
  const dataHome = path.join(root, "data");
  const previousXdg = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = dataHome;
  t.after(() => {
    if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousXdg;
    fs.rmSync(root, { recursive: true, force: true });
  });
  fs.mkdirSync(path.join(installRoot, "versions"), { recursive: true });
  fs.mkdirSync(path.dirname(cli), { recursive: true });
  fs.writeFileSync(path.join(installRoot, ".hlsr-install.json"), '{"schema":1}\n');
  fs.writeFileSync(path.join(installRoot, "start"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  fs.writeFileSync(cli, "my custom launcher", { mode: 0o755 });
  assert.equal(ensureManagedCli({ installRoot, home }), false);
  assert.equal(fs.readFileSync(cli, "utf8"), "my custom launcher");
  fs.unlinkSync(cli);
  if (NO_SYMLINKS) return t.skip(NO_SYMLINKS);
  const foreignTarget = path.join(root, "not-installed", "other-launcher");
  fs.symlinkSync(foreignTarget, cli);
  assert.equal(ensureManagedCli({ installRoot, home }), false);
  assert.equal(managedCliPath(installRoot, { home }), null);
  assert.equal(fs.readlinkSync(cli), foreignTarget);
  const result = require("../src/main/uninstall/launcher-uninstall").uninstallLauncher({ installRoot, home, libraryRoot: path.join(root, "library") });
  assert.equal(result.status, "ok");
  assert.equal(result.cliRemoved, false);
  assert.equal(fs.readlinkSync(cli), foreignTarget);
});

test("CLI: удаляется только команда конкретной управляемой установки", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cli-remove-"));
  const home = path.join(root, "home");
  const installRoot = path.join(root, "install");
  const previousXdg = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = path.join(root, "data");
  t.after(() => {
    if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousXdg;
    fs.rmSync(root, { recursive: true, force: true });
  });
  fs.mkdirSync(path.join(installRoot, "versions"), { recursive: true });
  fs.writeFileSync(path.join(installRoot, ".hlsr-install.json"), '{"schema":1}\n');
  fs.writeFileSync(path.join(installRoot, "start"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  assert.equal(ensureManagedCli({ installRoot, home }), true);
  assert.equal(managedCliPath(path.join(root, "different-install"), { home }), null);
  const result = require("../src/main/uninstall/launcher-uninstall").uninstallLauncher({ installRoot, home, libraryRoot: path.join(root, "library") });
  assert.equal(result.status, "ok");
  assert.equal(result.cliRemoved, true);
  assert.equal(fs.existsSync(path.join(home, ".local/bin/hlsr")), false);
});

test("Uninstall: игры удаляются только с явного согласия", (t) => {
  // Регресс «Часть файлов осталась: .../app.asar/dist/...»: внутри Electron
  // патченный fs считает *.asar каталогом-архивом, и рекурсивное удаление
  // билось о виртуальные пути, оставляя настоящий app.asar на диске.
  for (const name of ["uninstall/launcher-uninstall.js", "uninstall/library-cleanup.js", "linux/library-cleanup.js"]) {
    const text = fs.readFileSync(path.join(__dirname, "../src/main", name), "utf8");
    assert.ok(!/require\(["']fs["']\)/.test(text), `${name} обязан использовать fs-compat вместо патченного fs`);
  }
  const removal = fs.readFileSync(path.join(__dirname, "../src/main/game-delivery/game-removal.js"), "utf8");
  assert.ok(!/require\(["']fs["']\)/.test(removal), "game-removal.js обязан использовать fs-compat");
  const uninstaller = require("../src/main/uninstall/launcher-uninstall");
  const cleanup = require("../src/main/linux/library-cleanup");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-uninstall-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const dataHome = path.join(root, "data");
  const previousXdg = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = dataHome;
  t.after(() => {
    if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousXdg;
  });

  function fixture() {
    const tag = `${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6)}`;
    const installRoot = path.join(root, `opt-${tag}`, "hlsr");
    fs.mkdirSync(path.join(installRoot, "versions", "0.0.1", "resources"), { recursive: true });
    fs.writeFileSync(path.join(installRoot, ".hlsr-install.json"), JSON.stringify({ schema: 1 }));
    fs.writeFileSync(path.join(installRoot, "versions", "0.0.1", "hlsr"), "binary");
    // Файл с суффиксом .asar внутри установки: под Electron его нельзя
    // отдавать патченному fs (увидит «каталог» и не удалит настоящий файл).
    fs.writeFileSync(path.join(installRoot, "versions", "0.0.1", "resources", "app.asar"), "fake-asar-bytes");
    const library = path.join(root, `lib-${tag}`, "HLSR");
    fs.mkdirSync(path.join(library, "Games", "ghosting-renewed"), { recursive: true });
    fs.writeFileSync(path.join(library, "Games", "ghosting-renewed", "installed.json"), JSON.stringify({ gameId: "ghosting-renewed" }));
    const desktopFile = path.join(dataHome, "applications", "hlsr.desktop");
    fs.mkdirSync(path.dirname(desktopFile), { recursive: true });
    fs.writeFileSync(desktopFile, "[Desktop Entry]\nName=HLSR\nX-HLSR-Managed=true\n");
    const iconFile = path.join(dataHome, "icons", "hicolor", "256x256", "apps", "hlsr-appimage.png");
    fs.mkdirSync(path.dirname(iconFile), { recursive: true });
    fs.writeFileSync(iconFile, "icon");
    return { installRoot, library, desktopFile, iconFile };
  }

  // План видит размеры, игры по умолчанию остаются.
  const first = fixture();
  const plan = uninstaller.planLauncherUninstall({ installRoot: first.installRoot, libraryRoot: first.library });
  assert.equal(plan.managed, true);
  assert.ok(plan.installBytes > 0);
  assert.equal(plan.libraryExists, true);
  assert.ok(plan.libraryBytes > 0);
  assert.equal(plan.desktopEntry, first.desktopFile);
  const kept = uninstaller.uninstallLauncher({ installRoot: first.installRoot, libraryRoot: first.library });
  assert.equal(kept.status, "ok");
  assert.equal(kept.gamesRemoved, false);
  assert.equal(fs.existsSync(first.installRoot), false);
  assert.equal(fs.existsSync(first.desktopFile), false);
  assert.equal(fs.existsSync(first.iconFile), false);
  assert.equal(fs.existsSync(path.join(first.library, "Games", "ghosting-renewed", "installed.json")), true);

  // С согласием и маркером библиотека уходит целиком.
  const second = fixture();
  cleanup.writeMarker(second.library);
  const wiped = uninstaller.uninstallLauncher({
    installRoot: second.installRoot, libraryRoot: second.library, removeGames: true,
  });
  assert.equal(wiped.status, "ok");
  assert.equal(wiped.gamesRemoved, true);
  assert.equal(fs.existsSync(second.library), false);

  // Без маркера чужое в корне библиотеки не трогаем.
  const third = fixture();
  const foreign = path.join(third.library, "my-saves.txt");
  fs.writeFileSync(foreign, "precious");
  const partial = uninstaller.uninstallLauncher({
    installRoot: third.installRoot, libraryRoot: third.library, removeGames: true,
  });
  assert.equal(partial.gamesRemoved, true);
  assert.equal(fs.existsSync(foreign), true);
  assert.equal(fs.existsSync(path.join(third.library, "Games")), false);

  // Неуправляемый каталог и домашний каталог удалять отказываемся.
  const plain = path.join(root, "plain");
  fs.mkdirSync(plain, { recursive: true });
  assert.throws(
    () => uninstaller.planLauncherUninstall({ installRoot: plain, libraryRoot: path.join(root, "lib-x") }),
    (error) => error.code === "NOT_MANAGED"
  );
  assert.throws(
    () => uninstaller.planLauncherUninstall({ installRoot: plain, libraryRoot: os.homedir() }),
    (error) => error.code === "NOT_MANAGED" || error.code === "PATH_INVALID"
  );
});

test("Proton: HLSR принимает только закреплённый проверенный архив", () => {
  const pinned = ProtonManager.PINNED_ARCHIVE;
  const archive = {
    name: pinned.name,
    browser_download_url: "https://example.test/archive",
    size: pinned.size,
    digest: `sha256:${pinned.sha256}`,
  };
  const release = { tag_name: ProtonManager.RELEASE_TAG, assets: [archive] };
  assert.deepEqual(ProtonManager.safeReleaseAsset(release), { archive });
  assert.throws(() => ProtonManager.safeReleaseAsset({ ...release, tag_name: "latest" }));
  assert.throws(() => ProtonManager.safeReleaseAsset({ ...release, assets: [{ ...archive, size: archive.size + 1 }] }));
  assert.throws(() => ProtonManager.safeReleaseAsset({ ...release, assets: [{ ...archive, digest: `sha256:${"0".repeat(64)}` }] }));
});

test("Proton: архив допускает только внутренние символические ссылки", () => {
  const root = ProtonManager.PINNED_ARCHIVE.name.replace(/\.tar\.xz$/, "");
  const names = `${root}/\n${root}/proton\n${root}/lib/link\n`;
  const safe = [
    `drwxr-xr-x 0 0 0 0 Sep 14 00:00 ${root}/`,
    `-rwxr-xr-x 0 0 0 1 Sep 14 00:00 ${root}/proton`,
    `lrwxrwxrwx 0 0 0 0 Sep 14 00:00 ${root}/lib/link -> ../proton`,
  ].join("\n");
  assert.equal(ProtonManager.validateArchiveListing(names, safe, root).length, 3);
  assert.throws(() => ProtonManager.validateArchiveListing(
    names, safe.replace("../proton", "../../outside"), root));
  assert.throws(() => ProtonManager.validateArchiveListing(
    names, safe.replace("lrwxrwxrwx", "hrwxrwxrwx"), root));
});

test("Proton: HLSR переводит управляемый CachyOS на Wineland, сохраняя ручной выбор", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-proton-migration-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const managed = path.join(root, "protons");
  const legacy = path.join(managed, "proton-cachyos-11.0-20260703-slr-x86_64");
  const upstream = path.join(managed, "proton-wineland-11.0-20260913-x86_64");
  const wineland = path.join(managed, "proton-wineland-11.0-20260913-hlsr2-x86_64");
  const otherManaged = path.join(managed, "Proton-Test");
  const custom = path.join(root, "custom", "GE-Proton");
  for (const directory of [legacy, upstream, wineland, otherManaged, custom]) {
    fs.mkdirSync(directory, { recursive: true });
    fs.writeFileSync(path.join(directory, "proton"), "test", { mode: 0o755 });
  }
  const manager = new ProtonManager({ dataDir: root, home: root });
  manager.select(legacy);
  assert.equal((await manager.ensureReference()).path, wineland);
  assert.equal(manager.selectedPath(), wineland);
  manager.select(upstream);
  assert.equal((await manager.ensureReference()).path, wineland);
  assert.equal(manager.selectedPath(), wineland);
  manager.addCustom(custom);
  manager.select(custom);
  assert.equal((await manager.ensureReference()).path, wineland);
  assert.equal(manager.selectedPath(), custom);
  manager.select(otherManaged);
  assert.equal((await manager.ensureReference()).path, wineland);
  assert.equal(manager.selectedPath(), otherManaged);
});

test("Proton: старая управляемая сборка не запускается, пока HLSR-форк скачивается", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-proton-gate-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const legacy = path.join(root, "protons", "proton-wineland-11.0-20260913-x86_64");
  fs.mkdirSync(legacy, { recursive: true });
  fs.writeFileSync(path.join(legacy, "proton"), "test", { mode: 0o755 });
  const manager = new ProtonManager({ dataDir: root, home: root });
  manager.writeSelection({ path: legacy, customPaths: [] });
  assert.equal(manager.selected(), null);
});

test("Proton: Wineland включает Wayland и локальный shader cache только в подходящей сессии", () => {
  const root = path.join(os.tmpdir(), "hlsr-wineland-env");
  const proton = { path: "/tmp/proton-wineland", name: "Proton Wineland" };
  const manager = { selected: () => proton };
  const wayland = new ProtonRunner({ manager, dataDir: root, resourcesPath: "/tmp" });
  wayland.umuPath = () => "/usr/bin/umu-run";
  const previous = process.env.WAYLAND_DISPLAY;
  const previousWayland = process.env.PROTON_ENABLE_WAYLAND;
  const previousCache = process.env.PROTON_LOCAL_SHADER_CACHE;
  delete process.env.PROTON_ENABLE_WAYLAND;
  delete process.env.PROTON_LOCAL_SHADER_CACHE;
  process.env.WAYLAND_DISPLAY = "wayland-0";
  try {
    const env = wayland.environment("ghosting-renewed").env;
    assert.equal(env.PROTON_ENABLE_WAYLAND, "1");
    assert.equal(env.PROTON_LOCAL_SHADER_CACHE, "1");
    delete process.env.WAYLAND_DISPLAY;
    assert.equal(wayland.environment("ghosting-renewed").env.PROTON_ENABLE_WAYLAND, "0");
    process.env.WAYLAND_DISPLAY = "wayland-0";
    process.env.PROTON_ENABLE_WAYLAND = "0";
    process.env.PROTON_LOCAL_SHADER_CACHE = "0";
    const overridden = wayland.environment("ghosting-renewed").env;
    assert.equal(overridden.PROTON_ENABLE_WAYLAND, "0");
    assert.equal(overridden.PROTON_LOCAL_SHADER_CACHE, "0");
    // HL1 идёт через X11: всё в её префиксе получает тот же режим, иначе
    // LiveSplit, запущенный первым, навязал бы hl.exe winewayland без EGL.
    process.env.PROTON_ENABLE_WAYLAND = "1";
    assert.equal(wayland.environment("hl1-won").env.PROTON_ENABLE_WAYLAND, "0");
    delete process.env.PROTON_ENABLE_WAYLAND;
    assert.equal(wayland.environment("hl1-won").env.PROTON_ENABLE_WAYLAND, "0");
    assert.equal(wayland.environment("hl2-4260").env.PROTON_ENABLE_WAYLAND, "1");
  } finally {
    if (previous === undefined) delete process.env.WAYLAND_DISPLAY;
    else process.env.WAYLAND_DISPLAY = previous;
    if (previousWayland === undefined) delete process.env.PROTON_ENABLE_WAYLAND;
    else process.env.PROTON_ENABLE_WAYLAND = previousWayland;
    if (previousCache === undefined) delete process.env.PROTON_LOCAL_SHADER_CACHE;
    else process.env.PROTON_LOCAL_SHADER_CACHE = previousCache;
  }
});

test("Steam friends: Linux account API reads the selected user's friends and correct IDs", (t) => {
  const SteamAccount = require("../src/main/linux/SteamAccount");
  const friends = require("../src/main/profile/steam-friends");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-friends-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "config"), { recursive: true });
  fs.writeFileSync(path.join(root, "config/loginusers.vdf"),
    '"users" { "76561197960265770" { "MostRecent" "1" "PersonaName" "Owner" } }');
  const config = friends.localConfigPath(root, 42);
  fs.mkdirSync(path.dirname(config), { recursive: true });
  fs.writeFileSync(config, '"UserLocalConfigStore" { "friends" { "42" { "name" "Owner" } "123" { "name" "Friend" } } }');
  const account = new SteamAccount({ locator: { locateSteam: () => ({ installPath: root }) } });
  assert.equal(account.activeAccountId(), 42);
  assert.equal(account.describe().steamId2, "STEAM_0:0:21");
  const result = friends.list({ installPath: root, accountId: account.activeAccountId(), avatars: false });
  assert.equal(result.length, 1);
  assert.equal(result[0].personaName, "Friend");
  assert.equal(result[0].steamId64, "76561197960265851");
  assert.equal(result[0].steamId2, "STEAM_0:1:61");
  assert.equal(SteamAccount.MAX_AVATAR_BYTES, 512 * 1024);
});

test("Preload: all four game menu actions reach IPC, unknown channels remain blocked", async () => {
  const vm = require("node:vm");
  let bridge;
  const calls = [];
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../src/main/preload.js"), "utf8"), {
    process: { platform: "linux" },
    require: () => ({
      contextBridge: { exposeInMainWorld: (_name, value) => { bridge = value; } },
      clipboard: {},
      ipcRenderer: {
        sendSync: () => "",
        invoke: async (...args) => { calls.push(args); return { status: "ok" }; },
      },
    }),
  });
  for (const action of ["create-shortcut", "open-screenshots", "open-folder", "check-updates"]) {
    const channel = `game-menu-${action}`;
    await bridge.ipcRenderer.invoke(channel, "ghosting-renewed");
    assert.deepEqual(calls[calls.length - 1], [channel, "ghosting-renewed"]);
  }
  assert.throws(() => bridge.ipcRenderer.invoke("untrusted-channel"), /Blocked IPC/);
});

test("Game shortcut: executable desktop entry uses a stable AppImage path and game argument", { skip: NOT_LINUX }, (t) => {
  const GameActions = require("../src/main/game-delivery/GameActions");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-shortcut-"));
  const previous = process.env.APPIMAGE;
  t.after(() => {
    if (previous === undefined) delete process.env.APPIMAGE;
    else process.env.APPIMAGE = previous;
    fs.rmSync(root, { recursive: true, force: true });
  });
  process.env.APPIMAGE = path.join(root, "HLSR 100%.AppImage");
  const actions = new GameActions({ electron: { app: { getPath: () => path.join(root, "Desktop") } } });
  const result = actions.createDesktopShortcut("ghosting-renewed");
  const entry = fs.readFileSync(result.shortcut, "utf8");
  assert.ok(entry.includes('HLSR 100%%.AppImage" --no-sandbox --select-game=ghosting-renewed'));
  assert.equal(fs.statSync(result.shortcut).mode & 0o777, 0o755);
  assert.equal(actions.removeDesktopShortcut("ghosting-renewed").removed, true);
});

test("a moved or copied library launches the game from where installed.json actually lives", () => {
  const InstallService = require("../src/main/game-delivery/InstallService");
  const { resolveLibrary, gameInstallPath } = require("../src/main/game-delivery/paths");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-moved-library-"));
  try {
    const paths = resolveLibrary(root);
    const here = gameInstallPath(paths, "ghosting-renewed");
    fs.mkdirSync(here, { recursive: true });
    fs.writeFileSync(path.join(here, "installed.json"), JSON.stringify({
      schemaVersion: 1, gameId: "ghosting-renewed", version: "1.0.10", state: "ready",
      installPath: "/old/library/Games/ghosting-renewed" }));
    const installed = new InstallService({ paths }).readInstalled("ghosting-renewed");
    assert.equal(installed.installPath, here);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a Proton game that exits with code 0 but leaves a fresh minidump counts as a crash", () => {
  const crashDumps = require("../src/main/game-delivery/crash-dumps");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-dump-"));
  try {
    fs.writeFileSync(path.join(root, "hl2_old.mdmp"), "old");
    const context = { installPath: root, snapshot: crashDumps.snapshotDumps(root, {}), startedAtMs: Date.now() };
    assert.equal(crashDumps.leftNewDump(context, {}), false);
    fs.writeFileSync(path.join(root, "hl2_20261005_235150_1_InvalidParameterHandler.mdmp"), "dump");
    assert.equal(crashDumps.leftNewDump(context, {}), true);
    assert.equal(crashDumps.leftNewDump(null, {}), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("Linux: без запущенного Steam Source-игра не стартует, лаунчер просит открыть Steam", async () => {
  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  const { DeliveryError } = require("../src/main/game-delivery/errors");
  const cancelled = [];
  const launches = [];
  const service = new LinuxLaunchService({
    delivery: {
      processService: {
        isRunning: () => false,
        markStarting() {},
        cancelStarting: (gameId, reason) => cancelled.push({ gameId, reason }),
      },
      isBusy: () => false,
      installedOf: () => { throw new Error("до установки доходить не должно"); },
    },
    protonRunner: { launch: async (...args) => { launches.push(args); throw new Error("Proton не должен стартовать"); } },
    steamLocator: {
      locateSteam: () => ({ installPath: "/steam" }),
      assertSteamRunning: () => { throw new DeliveryError("STEAM_NOT_RUNNING", "Steam не запущен"); },
    },
  });

  for (const gameId of ["ghosting-renewed", "hl2-4260", "hl2-2707"]) {
    await assert.rejects(service.launch(gameId), (error) => error.code === "STEAM_NOT_RUNNING", gameId);
    assert.ok(!service.launching.has(gameId), `${gameId}: флаг запуска снят`);
  }
  assert.equal(launches.length, 0);
  assert.deepEqual(cancelled.map((entry) => entry.gameId), ["ghosting-renewed", "hl2-4260", "hl2-2707"]);
});

test("Linux: шрифты LiveSplit ставятся до запуска игры, если таймер стартует вместе с ней", async (t) => {
  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  // Своя временная папка: поиск двойников по регистру читает настоящий диск, а рядом с /games
  // может лежать чужая Games (на Windows — C:\Games).
  const library = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-fonts-"));
  t.after(() => fs.rmSync(library, { recursive: true, force: true }));
  const run = async (settings, fontsFail = false) => {
    const order = [];
    const warnings = [];
    const service = new LinuxLaunchService({
      delivery: {
        processService: { isRunning: () => false, markStarting() {}, cancelStarting() {} },
        isBusy: () => false,
        installedOf: () => ({ installPath: path.join(library, "ghosting-renewed") }),
        registerStartedProcess: () => ({ state: "running" }),
      },
      protonRunner: {
        ensureLiveSplitFonts: async (gameId) => {
          order.push(`fonts:${gameId}`);
          if (fontsFail) throw new Error("regedit упал");
        },
      },
      steamLocator: { locateSteam: () => ({ installPath: "/steam" }), assertSteamRunning() {} },
      settingsStore: { get: () => settings },
      logger: { info() {}, warn: (message) => warnings.push(message), error() {} },
    });
    service.assertBuildMetadata = () => {};
    service.startSource = async () => {
      order.push("game");
      const child = new EventEmitter();
      child.pid = 4242;
      return { child, executablePath: "hl2.exe", proton: { name: "proton" }, prefix: "/pfx" };
    };
    await service.launch("ghosting-renewed");
    return { order, warnings };
  };

  assert.deepEqual((await run({ startTimer: true })).order, ["fonts:ghosting-renewed", "game"]);
  assert.deepEqual((await run({ startTimer: false })).order, ["game"], "без таймера префикс не трогается");
  assert.deepEqual((await run(null)).order, ["game"]);
  const failed = await run({ startTimer: true }, true);
  assert.deepEqual(failed.order, ["fonts:ghosting-renewed", "game"], "сбой шрифтов игру не останавливает");
  assert.ok(failed.warnings.some((message) => message.includes("шрифты LiveSplit")));
});

test("Linux: папка-двойник по регистру рядом с библиотекой находится до запуска игры", { skip: NO_CASE_SENSITIVITY }, async (t) => {
  const { findCaseConflict } = require("../src/main/linux/path-case");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-case-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const install = path.join(root, "Games", "HLSR", "Games", "ghosting-renewed");
  fs.mkdirSync(install, { recursive: true });

  assert.equal(findCaseConflict(install), null, "без двойников всё чисто");

  fs.mkdirSync(path.join(root, "Games", "hlsr"));
  const conflict = findCaseConflict(install);
  assert.deepEqual(conflict, { folder: path.join(root, "Games", "HLSR"), twin: path.join(root, "Games", "hlsr") });

  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  const launches = [];
  const service = new LinuxLaunchService({
    delivery: {
      processService: { isRunning: () => false, markStarting() {}, cancelStarting() {} },
      isBusy: () => false,
      installedOf: () => ({ installPath: install }),
    },
    protonRunner: { launch: async (...args) => { launches.push(args); throw new Error("Proton не должен стартовать"); } },
    steamLocator: { locateSteam: () => ({ installPath: "/steam" }), assertSteamRunning: () => ({ pid: 1 }) },
  });
  await assert.rejects(service.launch("ghosting-renewed"), (error) =>
    error.code === "LIBRARY_PATH_CASE_CONFLICT" && error.details.twin === conflict.twin);
  assert.equal(launches.length, 0);
});

test("канал presence на Linux слушает только 127.0.0.1 и не оставляет файл в текущей папке", async (t) => {
  const net = require("net");
  const { PresenceBridgeServer } = require("../src/main/game-presence/PresenceBridgeServer");
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-presence-cwd-"));
  const previous = process.cwd();
  process.chdir(cwd);
  const token = "a".repeat(64);
  const server = new PresenceBridgeServer({ platform: "linux", token });
  const sockets = [];
  t.after(() => {
    for (const socket of sockets) socket.destroy();
    server.stop();
    process.chdir(previous);
    fs.rmSync(cwd, { recursive: true, force: true });
  });

  const pipeName = server.start();
  assert.match(pipeName, /^\\\\\.\\pipe\\hlsr-presence-[0-9a-f]{16}$/);
  await new Promise((resolve) => server.server.once("listening", resolve));
  assert.equal(server.server.address().address, "127.0.0.1");
  const env = server.environment();
  assert.deepEqual(env, { HLSR_PRESENCE_PIPE: pipeName, HLSR_PRESENCE_TOKEN: token, HLSR_PRESENCE_PORT: String(server.port()) });
  assert.deepEqual(fs.readdirSync(cwd), []);

  const connect = () => new Promise((resolve, reject) => {
    const socket = net.connect(server.port(), "127.0.0.1", () => resolve(socket));
    socket.once("error", reject);
    socket.setEncoding("utf8");
    sockets.push(socket);
  });
  const nextLine = (socket) => new Promise((resolve) => {
    let buffer = "";
    socket.on("data", function onData(chunk) {
      buffer += chunk;
      if (buffer.indexOf("\n") === -1) return;
      socket.off("data", onData);
      resolve(JSON.parse(buffer.slice(0, buffer.indexOf("\n"))));
    });
  });

  const game = await connect();
  const ack = nextLine(game);
  game.write(JSON.stringify({ v: 1, type: "hello", token, session: "0123abcd" }) + "\n");
  assert.deepEqual(await ack, { v: 1, type: "discord", connected: false });
  assert.equal(server.diagnostics().connected, true);

  // Пока игра на связи, чужое соединение её не вытесняет.
  const stranger = await connect();
  await new Promise((resolve) => stranger.once("close", resolve));
  assert.equal(server.diagnostics().connected, true);
  assert.equal(server.setDiscordConnected(true), true);
  assert.deepEqual(await nextLine(game), { v: 1, type: "discord", connected: true });
});

test("мост presence под Proton: помощник стартует после игры, игра получает канал и токен, но не порт", async (t) => {
  const PresenceRelay = require("../src/main/linux/PresenceRelay");
  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-presence-relay-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const resources = path.join(root, "resources");
  fs.mkdirSync(path.join(resources, PresenceRelay.RESOURCE_DIRECTORY), { recursive: true });
  fs.writeFileSync(path.join(resources, PresenceRelay.RESOURCE_DIRECTORY, PresenceRelay.EXECUTABLE), "relay");
  const pipe = "\\\\.\\pipe\\hlsr-presence-0123456789abcdef";
  const environment = { HLSR_PRESENCE_PIPE: pipe, HLSR_PRESENCE_TOKEN: "b".repeat(64), HLSR_PRESENCE_PORT: "40123" };

  assert.equal(PresenceRelay.targetOf({}), null);
  assert.equal(PresenceRelay.targetOf(Object.assign({}, environment, { HLSR_PRESENCE_PIPE: "\\\\.\\pipe\\other" })), null);
  assert.equal(PresenceRelay.targetOf(Object.assign({}, environment, { HLSR_PRESENCE_PORT: "0" })), null);

  const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const launches = [], killed = [];
  let running = false;
  const makeService = (statusCode, extra = {}) => new LinuxLaunchService(Object.assign({
    presenceRelay: new PresenceRelay({ baseDir: path.join(root, "data"), resourcesPath: resources }),
    extraEnvProvider: () => environment,
    presenceWaitMs: 300,
    presenceSettleMs: 0,
    bxtPollMs: 5,
    processRunning: (name) => running && name === "hl2.exe",
    logger: { info() {}, warn() {}, error() {} },
    killProcessGroup: (pid) => killed.push(pid),
    protonRunner: {
      launch: async (gameId, file, args, options) => {
        const session = new EventEmitter();
        session.pid = 5151;
        session.unref = () => {};
        launches.push({ gameId, file, args, options });
        if (statusCode !== null) setTimeout(() => fs.writeFileSync(path.join(path.dirname(file), "relay.status"), String(statusCode)), 20);
        return { child: session };
      },
    },
  }, extra));

  // Только играм с client.dll, который знает про канал.
  assert.equal(makeService(0).presenceTarget({ executable: "hl2.exe" }), null);
  const service = makeService(0);
  const target = service.presenceTarget({ executable: "hl2.exe", presenceBridge: true });
  assert.deepEqual(target.gameEnv, { HLSR_PRESENCE_PIPE: pipe, HLSR_PRESENCE_TOKEN: "b".repeat(64) });

  // Пока процесса игры нет, помощник не запускается: общие процессы префикса
  // должна поднять игра.
  const game = new EventEmitter();
  const started = service.startPresenceRelay("ghosting-renewed", { executable: "hl2.exe", presenceBridge: true }, target, game);
  await delay(40);
  assert.equal(launches.length, 0);
  running = true;
  assert.equal(await started, true);
  assert.equal(launches.length, 1);
  assert.equal(path.basename(launches[0].file), PresenceRelay.EXECUTABLE);
  assert.ok(launches[0].file.startsWith(path.join(root, "data")), "помощник копируется в папку данных");
  assert.deepEqual(launches[0].args.slice(0, 6), ["--pipe", pipe, "--port", "40123", "--name", "hl2.exe"]);
  assert.ok(!launches[0].args.some((arg) => arg.includes("b".repeat(64))), "токен помощнику не передаётся");
  assert.equal(launches[0].options.env.PROTON_HIDE_PROCESS_WINDOW, "1");
  assert.deepEqual(killed, []);
  // Игра закрылась — сессия помощника тоже.
  game.emit("exit", 0);
  assert.deepEqual(killed, [5151]);

  // Игра вышла раньше, чем появилась, — помощник не нужен.
  running = false;
  launches.length = 0;
  const early = new EventEmitter();
  const skipped = makeService(0).startPresenceRelay("ghosting-renewed", { executable: "hl2.exe", presenceBridge: true }, target, early);
  early.emit("exit", 1);
  assert.equal(await skipped, false);
  assert.equal(launches.length, 0);

  // Помощник не ответил — игра всё равно идёт, его сессия закрывается.
  running = true;
  killed.length = 0;
  assert.equal(await makeService(null).startPresenceRelay("ghosting-renewed", { executable: "hl2.exe", presenceBridge: true }, target, new EventEmitter()), false);
  assert.deepEqual(killed, [5151]);
});

test("канал presence на Windows слушает и отдаёт окружение для игры", { timeout: 10000 }, async (t) => {
  const { PresenceBridgeServer } = require("../src/main/game-presence/PresenceBridgeServer");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-presence-"));
  // На Windows канал — только \\.\pipe\…, путь файла даёт EACCES; на Linux — сокет в папке теста.
  const pipeName = process.platform === "win32" ? `\\\\.\\pipe\\test-${path.basename(dir)}` : path.join(dir, "bridge.sock");
  const server = new PresenceBridgeServer({ platform: "win32", pipeName, token: "t".repeat(64) });
  t.after(() => {
    server.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  assert.equal(server.start(), pipeName);
  await new Promise((resolve, reject) => server.server.once("listening", resolve).once("error", reject));
  assert.deepEqual(server.environment(), { HLSR_PRESENCE_PIPE: pipeName, HLSR_PRESENCE_TOKEN: "t".repeat(64) });
});

test("Дампы: в общей CrashDumps считается только дамп exe этой игры, папки модов — только её", (t) => {
  const crashDumpsModule = require("../src/main/game-delivery/crash-dumps");
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-dumps-"));
  t.after(() => fs.rmSync(base, { recursive: true, force: true }));
  const install = path.join(base, "hl1-won");
  const wer = path.join(base, "LocalAppData", "CrashDumps");
  fs.mkdirSync(path.join(install, "Half-Life", "valve_WON"), { recursive: true });
  fs.mkdirSync(path.join(install, "ghostingmod"), { recursive: true });
  fs.mkdirSync(wer, { recursive: true });
  const env = { LOCALAPPDATA: path.join(base, "LocalAppData") };
  const before = crashDumpsModule.snapshotDumps(install, env, "hl1-won");
  const startedAt = Date.now();

  fs.writeFileSync(path.join(wer, "LiveSplit.exe.4242.dmp"), "x".repeat(10));
  fs.writeFileSync(path.join(install, "ghostingmod", "old.mdmp"), "x");
  assert.deepEqual(crashDumpsModule.findNewDumps(install, before, startedAt, env, "hl1-won"), [],
    "дамп LiveSplit и папка мода другой игры — не падение HL1");
  assert.equal(crashDumpsModule.leftNewDump({ installPath: install, snapshot: before, startedAtMs: startedAt, gameId: "hl1-won" }, env), false);

  fs.writeFileSync(path.join(wer, "hl.exe.777.dmp"), "x".repeat(20));
  const found = crashDumpsModule.findNewDumps(install, before, startedAt, env, "hl1-won");
  assert.deepEqual(found.map((dump) => path.basename(dump.path)), ["hl.exe.777.dmp"]);
});

test("Linux: Ghosting идёт через мост к Steam и видна в Steam под AppID 243730", () => {
  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  const { launchConfigOf } = require("../src/main/game-delivery/manifests");
  const service = new LinuxLaunchService({
    delivery: { processService: {} },
    protonRunner: {},
    steamLocator: { locateSteam: () => ({ installPath: "/steam" }), assertSteamRunning() {} },
    logger: { info() {}, warn() {}, error() {} },
  });
  const installed = { installPath: "/games/ghosting-renewed" };

  assert.deepEqual(service.steamBridgeOf(installed, launchConfigOf("ghosting-renewed")),
    { appId: 243730, steamPath: "/steam", installPath: "/games/ghosting-renewed" });
  assert.equal(service.steamBridgeOf(installed, launchConfigOf("hl2-4260")), null, "сборки HL2 пока идут через umu");
});

test("Linux: игра считается закрытой, когда пропал её процесс, даже если LiveSplit держит сессию Proton", async () => {
  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  const GameProcessService = require("../src/main/game-delivery/GameProcessService");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-watch-"));
  try {
    const processService = new GameProcessService({ paths: { state: root }, logger: { info() {}, warn() {}, error() {} } });
    const child = new EventEmitter();
    child.pid = 5150;
    child.exitCode = null;
    processService.registerStartedProcess("hl2-4260", {
      pid: child.pid, installPath: "/games/hl2", executablePath: "/games/hl2/hl2.exe", hostProcess: true,
      startedAt: new Date().toISOString(), child,
    });
    let alive = false;
    const seen = [];
    const service = new LinuxLaunchService({
      delivery: { processService },
      gameWatchMs: 5,
      gameProcessRunning: (name, gameId) => { seen.push(`${name}@${gameId}`); return alive; },
    });
    const finished = new Promise((resolve) => processService.once("finished", resolve));
    service.watchGameProcess("hl2-4260", service.configFor("hl2-4260"), child);

    // Пока процесс игры не появился, выход определяет только сессия.
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(processService.isRunning("hl2-4260"), true);
    alive = true;
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(processService.isRunning("hl2-4260"), true);
    alive = false;
    const result = await finished;
    assert.equal(result.gameId, "hl2-4260");
    assert.equal(result.lastExit.outcome, "exited");
    assert.equal(processService.isRunning("hl2-4260"), false);
    assert.equal(seen[0], "hl2.exe@hl2-4260");
    // Сессия Proton живёт дальше (LiveSplit), её выход позже ничего не ломает.
    child.emit("exit", 0, null);
    assert.equal(processService.isRunning("hl2-4260"), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
