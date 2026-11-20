"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { spawn } = require("child_process");
const { waitForPrefixCommand } = require("./PrefixCommand");
const PythonRuntime = require("./PythonRuntime");
const UserNamespaces = require("./UserNamespaces");
const { umuRoot } = require("./UserNamespaces");
const { DeliveryError } = require("../game-delivery/errors");
const { launchConfigOf } = require("../game-delivery/manifests");

const CORE_FONT_FILES = [
  "andalemo.ttf", "arial.ttf", "comic.ttf", "cour.ttf", "georgia.ttf",
  "impact.ttf", "times.ttf", "trebuc.ttf", "verdana.ttf", "webdings.ttf",
];

const LIVESPLIT_FONT_PROFILE = "livesplit";
const LIVESPLIT_FONT_MARKER = ".hlsr-livesplit-fonts-v1";
const LIVESPLIT_FONTS = [
  { source: "Carlito-Regular.ttf", target: "carlito.ttf", sha256: "f6418f708baede9789daef5d458c0f53d2a888af9820e8062934e504fedc6595" },
  { source: "Carlito-Bold.ttf", target: "carlitob.ttf", sha256: "bb5d20f79b82599ec72983597437373a80f2d2085fa91fc144fd74e876a594db" },
  { source: "Carlito-Italic.ttf", target: "carlitoi.ttf", sha256: "0b019225e58d702bfedcbd35c21696769f8ee115cb6343f84c2f240312450d1c" },
  { source: "Carlito-BoldItalic.ttf", target: "carlitobi.ttf", sha256: "b32928186c119599e03ca6a1ffc680fdcb7fac95772f4b95d989cf6cd3861517" },
];

// Steam Linux Runtime, который требует сборка Proton (toolmanifest.vdf).
const RUNTIME_BY_TOOL_APPID = {
  1628350: "steamrt3",
  4183110: "steamrt4",
};

// umu всегда выставляет UMU_ID, а Proton при нём отключает lsteamclient:
// игре, которой нужен живой Steam (старый steam.dll HL1), остаётся запуск
// напрямую через точку входа Steam Runtime, как это делает сам Steam.
const UMU_ONLY_ENV = ["GAMEID", "STORE", "PROTON_VERB", "WINEPREFIX", "UMU_ID", "UMU_USE_STEAM"];

function requiredRuntimeName(protonPath, fsModule = fs) {
  let text = "";
  try { text = fsModule.readFileSync(path.join(protonPath, "toolmanifest.vdf"), "utf8"); }
  catch (error) { return null; }
  const match = text.match(/"require_tool_appid"\s+"(\d+)"/);
  return match ? RUNTIME_BY_TOOL_APPID[match[1]] || null : null;
}

// Путь в виде Z:\... для аргументов Windows-программ: Wine отображает
// корень Linux на диск Z:.
function windowsPathOf(file) {
  return `Z:${path.resolve(file).replace(/\//g, "\\")}`;
}

function sha256File(file) {
  const hash = crypto.createHash("sha256");
  const content = fs.readFileSync(file);
  return hash.update(content).digest("hex");
}

function livesplitFontRegistry() {
  return [
    "REGEDIT4",
    "",
    "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts]",
    '"Carlito (TrueType)"="carlito.ttf"',
    '"Carlito Bold (TrueType)"="carlitob.ttf"',
    '"Carlito Italic (TrueType)"="carlitoi.ttf"',
    '"Carlito Bold Italic (TrueType)"="carlitobi.ttf"',
    "",
    "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\FontSubstitutes]",
    '"Calibri"="Carlito"',
    '"Segoe UI"="Carlito"',
    "",
  ].join("\r\n");
}

function executableInPath(name, env = process.env) {
  for (const dir of String(env.PATH || "").split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, name);
    try { fs.accessSync(candidate, fs.constants.X_OK); return candidate; } catch (error) { /* next */ }
  }
  return null;
}

function coreFontsStatus(prefix) {
  let logged = false;
  try { logged = fs.readFileSync(path.join(prefix, "winetricks.log"), "utf8").split(/\r?\n/).some((line) => line.trim() === "corefonts"); } catch (error) { /* fresh prefix */ }
  const directory = path.join(prefix, "drive_c", "windows", "Fonts");
  let names = [];
  try { names = fs.readdirSync(directory); } catch (error) { /* fresh prefix */ }
  const missing = CORE_FONT_FILES.filter((name) => {
    let fd;
    try {
      const actual = names.find((candidate) => candidate.toLowerCase() === name);
      if (!actual) return true;
      fd = fs.openSync(path.join(directory, actual), "r");
      if (fs.fstatSync(fd).size < 12) return true;
      const header = Buffer.alloc(4);
      if (fs.readSync(fd, header, 0, 4, 0) !== 4) return true;
      return header.readUInt32BE(0) !== 0x00010000 && !["OTTO", "true"].includes(header.toString("ascii"));
    } catch (error) { return true; }
    finally { if (fd !== undefined) fs.closeSync(fd); }
  });
  const marked = fs.existsSync(path.join(prefix, ".hlsr-corefonts-ready"));
  return { ready: (logged || marked) && !missing.length, logged, missing };
}

function allowedCpuListWithoutZero() {
  try {
    const match = fs.readFileSync("/proc/self/status", "utf8").match(/^Cpus_allowed_list:\s*([0-9,-]+)/m);
    if (!match) return null;
    const allowed = match[1].split(",").map((part) => {
      const bounds = part.split("-").map(Number);
      if (bounds.some((value) => !Number.isSafeInteger(value) || value < 0)) return null;
      const start = Math.max(1, bounds[0]);
      const end = bounds.length === 2 ? bounds[1] : bounds[0];
      return end < start ? null : start === end ? String(start) : `${start}-${end}`;
    }).filter(Boolean);
    return allowed.length ? allowed.join(",") : null;
  } catch (error) { return null; }
}

// Как Proton читает PROTON_NO_*: пустая строка и "0" — не заданы.
function protonFlag(value) {
  return value != null && String(value) !== "" && String(value) !== "0";
}

const HOST_WINESERVER_TIMEOUT_MS = 10000;

// umu ставит verb'ы winetricks из $PROTONPATH/protonfixes и делает эту папку
// рабочей. У сборок Valve её нет, и umu падает на FileNotFoundError.
function protonHasWinetricks(protonPath) {
  try { return fs.statSync(path.join(protonPath, "protonfixes", "winetricks")).isFile(); }
  catch (error) { return false; }
}

function sameFile(a, b) {
  return a.dev === b.dev && a.ino === b.ino;
}

// Сокет wineserver лежит в /tmp/.wine-<uid>/server-<dev>-<ino> по stat
// префикса, и сервер работает из этой папки. По ней находим сервер префикса,
// в том числе поднятый в контейнере Steam Runtime. Файлы сверяем по dev/ino
// через /proc/<pid>/{cwd,exe}: путь из чужого mount namespace может не совпасть
// с нашим, а stat по этим ссылкам всегда даёт сам файл.
function prefixWineservers(prefix, procRoot = "/proc", tmpRoot = "/tmp") {
  let serverDir;
  try {
    const stat = fs.statSync(prefix);
    const uid = typeof process.getuid === "function" ? process.getuid() : 0;
    serverDir = fs.statSync(path.join(tmpRoot, `.wine-${uid}`, `server-${stat.dev.toString(16)}-${stat.ino.toString(16)}`));
  } catch (error) { return []; }
  let entries = [];
  try { entries = fs.readdirSync(procRoot); } catch (error) { return []; }
  const servers = [];
  for (const pid of entries) {
    if (!/^\d+$/.test(pid)) continue;
    try {
      if (fs.readFileSync(path.join(procRoot, pid, "comm"), "utf8").trim() !== "wineserver") continue;
      if (!sameFile(fs.statSync(path.join(procRoot, pid, "cwd")), serverDir)) continue;
      const exe = fs.statSync(path.join(procRoot, pid, "exe"));
      let exePath = null;
      try { exePath = fs.readlinkSync(path.join(procRoot, pid, "exe")).replace(/ \(deleted\)$/, ""); }
      catch (error) { /* путь только для сообщения */ }
      servers.push({ pid: Number(pid), dev: exe.dev, ino: exe.ino, exePath });
    } catch (error) { /* процесс уже завершился или недоступен */ }
  }
  return servers;
}

function readText(file) {
  try { return fs.readFileSync(file, "utf8").trim(); } catch (error) { return null; }
}

const PREFIX_PROTON_MARKER = ".hlsr-prefix-proton";

// PROTON_ENABLE_WAYLAND из настроек запуска игры (manifests.js) или null.
function displayModeOf(gameId) {
  const config = launchConfigOf(gameId);
  const value = config && config.environment && config.environment.PROTON_ENABLE_WAYLAND;
  return typeof value === "string" ? value : null;
}

class ProtonRunner {
  constructor(options = {}) {
    this.manager = options.manager;
    this.dataDir = options.dataDir;
    this.resourcesPath = options.resourcesPath || process.resourcesPath;
    this.logger = options.logger || { info() {}, warn() {}, error() {} };
    this.spawnFn = options.spawnFn || spawn;
    this.prefixCommandTimeoutMs = options.prefixCommandTimeoutMs;
    this.prefixCommandKillGraceMs = options.prefixCommandKillGraceMs;
    this.fontsPending = new Map();
    this.prefixPending = new Map();
    this.pythonRuntime = options.pythonRuntime || new PythonRuntime({ dataDir: this.dataDir, logger: this.logger });
    this.userNamespaces = options.userNamespaces || new UserNamespaces({ resourcesPath: this.resourcesPath, logger: this.logger });
    this.displayModeOf = options.displayModeOf || displayModeOf;
    this.prefixWineservers = options.prefixWineservers || prefixWineservers;
  }

  umuPath() {
    const bundled = path.join(this.resourcesPath, "umu", "umu-run");
    try { fs.accessSync(bundled, fs.constants.X_OK); return bundled; } catch (error) { /* use host */ }
    return executableInPath("umu-run");
  }

  prefixFor(gameId) {
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(String(gameId || "")))
      throw new Error("Недопустимый идентификатор игры для Proton-префикса");
    return path.join(this.dataDir, "compatdata", gameId, "pfx");
  }

  environment(gameId) {
    const proton = this.manager.selected();
    if (!proton) throw new Error("Proton не установлен. Откройте настройки и скачайте Proton Wineland.");
    const umu = this.umuPath();
    if (!umu) throw new Error("umu-run не найден. Пересоберите AppImage с umu или установите umu-launcher.");
    const prefix = this.prefixFor(gameId);
    fs.mkdirSync(path.dirname(prefix), { recursive: true });
    const env = Object.assign({}, process.env);
    for (const key of [
      "LD_LIBRARY_PATH", "LD_PRELOAD", "PYTHONPATH", "PYTHONHOME", "QT_PLUGIN_PATH",
      "APPDIR", "STEAM_COMPAT_DATA_PATH", "STEAM_COMPAT_CLIENT_INSTALL_PATH",
    ]) delete env[key];
    if (process.env.APPDIR)
      env.PATH = String(env.PATH || "").split(path.delimiter)
        .filter((part) => !part.startsWith(process.env.APPDIR)).join(path.delimiter);
    // Wineland enables Wayland itself; spelling it out here makes HLSR's
    // default deterministic while still allowing PROTON_ENABLE_WAYLAND=0 as
    // an explicit troubleshooting override.
    if (env.PROTON_ENABLE_WAYLAND == null)
      env.PROTON_ENABLE_WAYLAND = env.WAYLAND_DISPLAY ? "1" : "0";
    // Графический драйвер выбирает explorer.exe первой программы в префиксе,
    // и он остаётся у всех остальных. Поэтому всё в префиксе игры идёт в её
    // режиме дисплея. HL1 работает через X11: если первым стартовал LiveSplit
    // с Wayland, hl.exe получал winewayland без EGL и падал на
    // «ChoosePixelFormat failed».
    const display = this.displayModeOf(gameId);
    if (display != null) env.PROTON_ENABLE_WAYLAND = display;
    // Keep a cache beside this game's prefix to reduce repeat shader stutter.
    if (env.PROTON_LOCAL_SHADER_CACHE == null) env.PROTON_LOCAL_SHADER_CACHE = "1";
    return {
      command: umu,
      prefix,
      proton,
      env: Object.assign(env, {
        WINEPREFIX: prefix,
        PROTONPATH: proton.path,
        // umu defaults to waitforexitandrun, which blocks LiveSplit until the game exits.
        PROTON_VERB: "run",
        GAMEID: "0",
        STORE: "none",
      }),
    };
  }

  // Wineserver префикса поднимается на хосте, вне контейнеров Steam Runtime.
  // Каждый запуск umu идёт в своём контейнере со своим user namespace, а
  // wineserver читает память процессов префикса через /proc/<pid>/mem. Если
  // сервер поднял контейнер LiveSplit, память игры из соседнего контейнера ему
  // закрыта: EnumProcessModules отвечает «доступ запрещён», и SourceSplit не
  // находит игру. Хост владеет namespace всех контейнеров, его сервер читает
  // память каждого. Сервер, который уже работает, остаётся как есть.
  async ensureHostWineserver(launch, environment) {
    const server = path.join(launch.proton.path, "files", "bin", "wineserver");
    try { fs.accessSync(server, fs.constants.X_OK); }
    catch (error) { return "unavailable"; }
    // Новый префикс создаёт сам Proton.
    if (!fs.existsSync(path.join(launch.prefix, "system.reg"))) return "no-prefix";
    this.assertPrefixServerMatches(launch, server);
    const env = {
      PATH: "/usr/bin:/bin",
      HOME: environment.HOME || os.homedir(),
      WINEPREFIX: launch.prefix,
      // Синхронизация сервера должна совпасть с клиентами: Proton выставляет
      // WINEFSYNC так же, а PROTON_NO_NTSYNC сервер читает сам.
      WINEFSYNC: protonFlag(environment.PROTON_NO_FSYNC) ? "0" : "1",
    };
    if (environment.PROTON_NO_NTSYNC != null) env.PROTON_NO_NTSYNC = environment.PROTON_NO_NTSYNC;
    const code = await new Promise((resolve) => {
      let child;
      try {
        child = this.spawnFn(server, [], { env, stdio: "ignore", detached: true, shell: false });
      } catch (error) { resolve(null); return; }
      // Без -f wineserver уходит в фон, когда сокет готов: родитель
      // завершается сразу (0 — запущен, 2 — сервер префикса уже работает).
      const timer = setTimeout(() => resolve(null), HOST_WINESERVER_TIMEOUT_MS);
      child.once("error", () => { clearTimeout(timer); resolve(null); });
      child.once("exit", (exitCode) => { clearTimeout(timer); resolve(exitCode); });
    });
    if (code === 0) {
      this.logger.info(`[proton] wineserver префикса запущен на хосте: ${launch.prefix}`);
      return "started";
    }
    if (code === 2) return "running";
    this.logger.warn(`[proton] wineserver на хосте не запустился (код ${code}); его поднимет контейнер`);
    return "failed";
  }

  // Клиенты Wine работают только с сервером своей сборки. Если Proton сменили,
  // пока в префиксе открыт LiveSplit, игра из нового Proton выходит с кодом 1,
  // а wineboot -u висит до таймаута. Чужой сервер не гасим: в нём могут
  // работать программы игрока, поэтому просим их закрыть.
  assertPrefixServerMatches(launch, server) {
    let own;
    try { own = fs.statSync(server); } catch (error) { return; }
    const foreign = this.prefixWineservers(launch.prefix).find((item) => !sameFile(item, own));
    if (!foreign) return;
    const match = foreign.exePath && foreign.exePath.match(/([^/]+)\/files\/bin\/wineserver$/);
    const other = match ? match[1] : "другой сборки";
    throw new DeliveryError("LAUNCH_FAILED",
      `В префиксе ещё работает Wine из Proton ${other} (wineserver, PID ${foreign.pid}), а выбран ${launch.proton.name}. ` +
      "Закройте LiveSplit и другие программы этой игры и запустите снова.",
      { pid: foreign.pid, proton: other });
  }

  async runPrefixCommand(gameId, args, environment) {
    const launch = this.environment(gameId);
    launch.env = await this.pythonRuntime.prepareEnvironment(launch.env);
    await this.userNamespaces.ensure(launch.env);
    await this.ensureHostWineserver(launch, Object.assign({}, launch.env, environment || {}));
    const run = () => {
      const child = this.spawnFn(launch.command, args, {
        env: Object.assign(launch.env, environment || {}),
        stdio: ["ignore", "pipe", "pipe"],
        detached: true,
        shell: false,
      });
      return waitForPrefixCommand(child, {
        label: `umu-run ${args.join(" ")}`,
        timeoutMs: this.prefixCommandTimeoutMs,
        killGraceMs: this.prefixCommandKillGraceMs,
      });
    };
    try { return await run(); }
    catch (error) {
      // The first umu call downloads bwrap, so preflight cannot test it yet.
      if (!/bwrap:.*(?:Permission denied|Operation not permitted)/i.test(error.stderr || "")) throw error;
      await this.userNamespaces.ensure(launch.env);
      return run();
    }
  }

  prefixNeedsProtonUpdate(prefix, protonPath) {
    const managedRoot = path.join(this.dataDir, "protons") + path.sep;
    const selectedRoot = path.resolve(protonPath) + path.sep;
    const directories = [path.join(prefix, "drive_c")];
    while (directories.length) {
      const directory = directories.pop();
      let entries;
      try { entries = fs.readdirSync(directory, { withFileTypes: true }); }
      catch (error) { if (error.code === "ENOENT") continue; throw error; }
      for (const entry of entries) {
        const file = path.join(directory, entry.name);
        if (entry.isDirectory()) directories.push(file);
        else if (entry.isSymbolicLink()) {
          const target = path.resolve(directory, fs.readlinkSync(file));
          if (!target.startsWith(managedRoot) || target.startsWith(selectedRoot)) continue;
          const relative = target.slice(managedRoot.length).split(path.sep).slice(1).join(path.sep);
          // Wine может оставить ссылку на библиотеку, которую новая сборка
          // больше не поставляет (например OpenCL). Она не требует миграции.
          if (relative && fs.existsSync(path.join(protonPath, relative))) return true;
        }
      }
    }
    return false;
  }

  async ensurePrefixRuntime(gameId, environment) {
    if (this.prefixPending.has(gameId)) return this.prefixPending.get(gameId);
    const proton = this.manager.selected();
    const prefix = this.prefixFor(gameId);
    if (!proton || !this.prefixNeedsProtonUpdate(prefix, proton.path)) return;
    // Чужой Proton (у игрока — Proton Experimental) после wineboot -u может
    // оставить часть ссылок на прежнюю сборку, и проверка выше срабатывала бы
    // на каждом запуске. Отметка помнит, что этот Proton уже готовил префикс;
    // если с тех пор префикс обновлял кто-то другой, меняется его version.
    const marker = path.join(prefix, PREFIX_PROTON_MARKER);
    const current = () => JSON.stringify({
      proton: (() => { try { return fs.realpathSync(proton.path); } catch (error) { return proton.path; } })(),
      protonVersion: readText(path.join(proton.path, "version")),
      prefixVersion: readText(path.join(prefix, "version")),
    });
    if (readText(marker) === current()) return;
    // Proton сам обновляет свои DLL, реестр и ссылки. Это нужно выполнить
    // до проверки шрифтов и запуска игры, в том числе в режиме моста Steam.
    const pending = this.runPrefixCommand(gameId, ["wineboot.exe", "-u"], environment)
      .then(() => {
        try { fs.writeFileSync(marker, `${current()}\n`, { mode: 0o600 }); }
        catch (error) { this.logger.warn(`[proton] отметка подготовки префикса не записана: ${error.message}`); }
        this.logger.info(`[proton] префикс ${gameId} подготовлен для ${proton.name}`);
      })
      .finally(() => this.prefixPending.delete(gameId));
    this.prefixPending.set(gameId, pending);
    return pending;
  }

  async ensureCoreFonts(gameId, environment) {
    const prefix = this.prefixFor(gameId);
    const marker = path.join(prefix, ".hlsr-corefonts-ready");
    const pendingKey = `core:${gameId}`;
    if (this.fontsPending.has(pendingKey)) return this.fontsPending.get(pendingKey);
    const task = (async () => {
      const status = coreFontsStatus(prefix);
      if (status.ready) {
        // umu 1.4.4 exits 1 for an already-installed verb. A missing HLSR
        // marker after migration must not turn a healthy prefix into a failure.
        if (!fs.existsSync(marker)) fs.writeFileSync(marker, `${new Date().toISOString()}\n`, { mode: 0o600 });
        return;
      }
      if (status.logged) {
        throw new Error(`В префиксе corefonts отмечены установленными, но повреждены или отсутствуют: ${status.missing.join(", ")}. Префикс: ${prefix}`);
      }
      const proton = this.manager.selected();
      if (proton && !protonHasWinetricks(proton.path))
        throw new DeliveryError("LAUNCH_FAILED",
          `В Proton ${proton.name} нет protonfixes/winetricks, поэтому он не может установить шрифты в префикс игры. ` +
          "Для первого запуска выберите в настройках Proton Wineland (или GE-Proton); потом можно вернуться к прежнему.",
          { proton: proton.name });
      this.logger.info(`[proton] устанавливаю corefonts в ${prefix}`);
      for (let attempt = 0; attempt < 2; attempt++) {
        let output;
        try {
          output = await this.runPrefixCommand(gameId, ["winetricks", "corefonts"], environment);
        } catch (error) {
          // Accept only the specific, verified concurrent-install case.
          const alreadyInstalled = error.exitCode === 1 && /winetricks verb ['"]corefonts['"] is already installed/.test(`${error.stderr || ""}\n${error.stdout || ""}`);
          if (!alreadyInstalled || !coreFontsStatus(prefix).ready) throw error;
        }
        const installed = coreFontsStatus(prefix);
        if (!installed.missing.length) break;
        // On a cold prefix Proton can exit 0 after initializing Wine without
        // completing winetricks. Retry once; never mark partial fonts ready.
        if (attempt === 0 && !installed.logged) {
          this.logger.warn("[proton] corefonts не завершены после инициализации префикса; повторяю установку");
          continue;
        }
        const details = output ? [output.stderr, output.stdout].filter(Boolean).join("\n") : "";
        throw new Error(`Установка corefonts завершилась без необходимых шрифтов: ${installed.missing.join(", ")}. Префикс: ${prefix}${details ? `\n${details}` : ""}`);
      }
      fs.mkdirSync(prefix, { recursive: true });
      fs.writeFileSync(marker, `${new Date().toISOString()}\n`, { mode: 0o600 });
    })().finally(() => this.fontsPending.delete(pendingKey));
    this.fontsPending.set(pendingKey, task);
    return task;
  }

  liveSplitFontsReady(prefix) {
    const marker = path.join(prefix, LIVESPLIT_FONT_MARKER);
    if (!fs.existsSync(marker)) return false;
    const fontDir = path.join(prefix, "drive_c", "windows", "Fonts");
    try {
      return LIVESPLIT_FONTS.every((font) =>
        sha256File(path.join(fontDir, font.target)) === font.sha256);
    } catch (error) { return false; }
  }

  async ensureLiveSplitFonts(gameId) {
    const prefix = this.prefixFor(gameId);
    if (this.liveSplitFontsReady(prefix)) return;
    const pendingKey = `livesplit:${gameId}`;
    if (this.fontsPending.has(pendingKey)) return this.fontsPending.get(pendingKey);
    const task = (async () => {
      const sourceDir = path.join(this.resourcesPath, "livesplit-fonts");
      const fontDir = path.join(prefix, "drive_c", "windows", "Fonts");
      const windowsTemp = path.join(prefix, "drive_c", "windows", "temp");
      fs.mkdirSync(fontDir, { recursive: true });
      fs.mkdirSync(windowsTemp, { recursive: true });
      for (const font of LIVESPLIT_FONTS) {
        const source = path.join(sourceDir, font.source);
        if (sha256File(source) !== font.sha256)
          throw new Error(`Шрифт LiveSplit повреждён: ${font.source}`);
        const target = path.join(fontDir, font.target);
        const temporary = `${target}.${process.pid}.tmp`;
        fs.copyFileSync(source, temporary);
        fs.chmodSync(temporary, 0o644);
        fs.renameSync(temporary, target);
      }
      const registryFile = path.join(windowsTemp, "hlsr-livesplit-fonts.reg");
      fs.writeFileSync(registryFile, livesplitFontRegistry(), { mode: 0o600 });
      try {
        await this.runPrefixCommand(gameId, [
          "regedit.exe", "/S", "C:\\windows\\temp\\hlsr-livesplit-fonts.reg",
        ], {
          PROTON_ENABLE_WAYLAND: this.environment(gameId).env.PROTON_ENABLE_WAYLAND,
          PROTON_USE_X11_EXCLUSIVE: "LiveSplit.exe",
        });
      } finally {
        try { fs.unlinkSync(registryFile); } catch (error) { /* already absent */ }
      }
      fs.writeFileSync(path.join(prefix, LIVESPLIT_FONT_MARKER),
        `${new Date().toISOString()}\n`, { mode: 0o600 });
      this.logger.info(`[proton] установлены шрифты LiveSplit в ${prefix}`);
    })().finally(() => this.fontsPending.delete(pendingKey));
    this.fontsPending.set(pendingKey, task);
    return task;
  }

  async ensureFonts(gameId, profile, environment) {
    await this.ensureCoreFonts(gameId, environment);
    if (profile === LIVESPLIT_FONT_PROFILE) await this.ensureLiveSplitFonts(gameId);
  }

  // Команда и окружение для запуска с мостом к клиенту Steam (см. UMU_ONLY_ENV).
  steamBridgeCommand(launch, executable, args, bridge) {
    if (!bridge || !Number.isInteger(bridge.appId) || bridge.appId <= 0)
      throw new Error("Для моста к Steam нужен AppID игры");
    if (!bridge.steamPath || !path.isAbsolute(bridge.steamPath))
      throw new Error("Для моста к Steam нужен путь к клиенту Steam");
    const runtime = requiredRuntimeName(launch.proton.path);
    if (!runtime) throw new Error(`Не удалось определить Steam Runtime для Proton ${launch.proton.name || launch.proton.path}`);
    const runtimeDir = path.join(umuRoot(launch.env), runtime);
    const entryPoint = path.join(runtimeDir, "_v2-entry-point");
    try { fs.accessSync(entryPoint, fs.constants.X_OK); }
    catch (error) { throw new Error(`Steam Runtime ${runtime} не найден: ${entryPoint}`, { cause: error }); }
    const env = Object.assign({}, launch.env);
    for (const key of UMU_ONLY_ENV) delete env[key];
    const appId = String(bridge.appId);
    const tools = `${launch.proton.path}:${runtimeDir}`;
    Object.assign(env, {
      STEAM_COMPAT_DATA_PATH: launch.prefix,
      STEAM_COMPAT_SHADER_PATH: path.join(launch.prefix, "shadercache"),
      STEAM_COMPAT_CLIENT_INSTALL_PATH: bridge.steamPath,
      STEAM_COMPAT_INSTALL_PATH: bridge.installPath || path.dirname(executable),
      STEAM_COMPAT_TOOL_PATHS: tools,
      STEAM_COMPAT_MOUNTS: tools,
      STEAM_COMPAT_APP_ID: appId,
      SteamAppId: appId,
      SteamGameId: appId,
    });
    return {
      command: entryPoint,
      argv: ["--verb=run", "--", path.join(launch.proton.path, "proton"), "run", executable, ...args],
      env,
    };
  }

  wrapperPath(name, env) {
    const found = path.isAbsolute(name)
      ? (() => { try { fs.accessSync(name, fs.constants.X_OK); return name; } catch (error) { return null; } })()
      : name.includes("/") ? null : executableInPath(name, env);
    if (!found)
      throw new DeliveryError("PROTON_WRAPPER_MISSING", `Программа из параметров Proton не найдена: ${name}`);
    return found;
  }

  async launch(gameId, executable, args = [], options = {}) {
    if (!path.isAbsolute(executable) || !fs.existsSync(executable))
      throw new Error(`Исполняемый файл не найден: ${executable}`);
    const throwIfAborted = () => {
      if (!options.signal || !options.signal.aborted) return;
      const error = new Error("Запуск отменён");
      error.name = "AbortError";
      error.code = "ABORT_ERR";
      throw error;
    };
    throwIfAborted();
    await this.ensurePrefixRuntime(gameId, options.env);
    throwIfAborted();
    if (!options.skipFonts) await this.ensureFonts(gameId, options.fontProfile, options.env);
    // Font setup is shared with regular game/timer launches. Cancelling a
    // preview must not kill that setup or spawn its game after setup completes.
    throwIfAborted();
    const launch = this.environment(gameId);
    launch.env = await this.pythonRuntime.prepareEnvironment(launch.env);
    await this.userNamespaces.ensure(launch.env);
    throwIfAborted();
    await this.ensureHostWineserver(launch, Object.assign({}, launch.env, options.env || {}));
    let command = launch.command;
    let argv = [executable, ...args];
    if (options.steamBridge) {
      const bridged = this.steamBridgeCommand(launch, executable, args, options.steamBridge);
      command = bridged.command;
      argv = bridged.argv;
      launch.env = bridged.env;
    }
    // Обёртка из параметров Proton игрока (mangohud, gamemoderun): как
    // «mangohud %command%» в Steam, она запускает umu-run целиком.
    if (Array.isArray(options.wrapper) && options.wrapper.length) {
      argv = [...options.wrapper.slice(1), command, ...argv];
      command = this.wrapperPath(options.wrapper[0], launch.env);
    }
    if (options.excludeCpu0 === true) {
      const taskset = executableInPath("taskset");
      const allowed = allowedCpuListWithoutZero();
      if (taskset && allowed) {
        argv = ["-c", allowed, command, ...argv];
        command = taskset;
      } else this.logger.warn("[proton] доступный набор CPU или taskset не найден; ограничение CPU 0 пропущено");
    }
    // Вывод umu, Proton и Wine идёт в файл: без него падение игры видно только
    // по коду выхода. Без PROTON_LOG Proton ставит WINEDEBUG=-all, так что
    // файл остаётся маленьким. Каждый запуск начинает его заново.
    let logFd = null;
    if (options.logFile) {
      try {
        fs.mkdirSync(path.dirname(options.logFile), { recursive: true });
        logFd = fs.openSync(options.logFile, "w", 0o600);
        fs.writeSync(logFd, `# ${new Date().toISOString()} ${launch.proton.name}: ${executable} ${args.join(" ")}\n`);
      } catch (error) {
        if (logFd !== null) try { fs.closeSync(logFd); } catch (closeError) { /* уже закрыт */ }
        logFd = null;
        this.logger.warn(`[proton] журнал запуска не открыт: ${error.message}`);
      }
    }
    let child;
    try {
      child = this.spawnFn(command, argv, {
        cwd: options.cwd || path.dirname(executable),
        env: Object.assign(launch.env, options.env || {}),
        detached: true,
        stdio: logFd === null ? "ignore" : ["ignore", logFd, logFd],
        shell: false,
      });
      await new Promise((resolve, reject) => {
        child.once("spawn", resolve);
        child.once("error", reject);
      });
    } finally {
      // Потомок держит свою копию дескриптора.
      if (logFd !== null) try { fs.closeSync(logFd); } catch (error) { /* уже закрыт */ }
    }
    return { child, proton: launch.proton, prefix: launch.prefix, command };
  }
}

module.exports = ProtonRunner;
module.exports.executableInPath = executableInPath;
module.exports.allowedCpuListWithoutZero = allowedCpuListWithoutZero;
module.exports.LIVESPLIT_FONT_PROFILE = LIVESPLIT_FONT_PROFILE;
module.exports.LIVESPLIT_FONT_MARKER = LIVESPLIT_FONT_MARKER;
module.exports.LIVESPLIT_FONTS = LIVESPLIT_FONTS;
module.exports.livesplitFontRegistry = livesplitFontRegistry;
module.exports.CORE_FONT_FILES = CORE_FONT_FILES;
module.exports.coreFontsStatus = coreFontsStatus;
module.exports.prefixWineservers = prefixWineservers;
module.exports.PREFIX_PROTON_MARKER = PREFIX_PROTON_MARKER;
module.exports.requiredRuntimeName = requiredRuntimeName;
module.exports.windowsPathOf = windowsPathOf;
module.exports.protonFlag = protonFlag;
module.exports.RUNTIME_BY_TOOL_APPID = RUNTIME_BY_TOOL_APPID;
