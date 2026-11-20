"use strict";

// Запуск собранного лаунчера в песочнице: свой профиль, своя библиотека,
// свой HOME (на Linux), порт отладки рендерера и хук главного процесса.
// Настоящие профиль, игры и настройки тестировщика не затрагиваются.

const fs = require("fs");
const path = require("path");
const net = require("net");
const { spawn, spawnSync } = require("child_process");
const { CdpSession, waitForTarget, delay } = require("./cdp");

const REPO = path.resolve(__dirname, "../../..");
const IS_WINDOWS = process.platform === "win32";

function defaultExecutable() {
  return IS_WINDOWS
    ? path.join(REPO, "build", "win-unpacked", "HLSR.exe")
    : path.join(REPO, "build", "linux-unpacked", "hlsr");
}

// Релизная сборка закрыта fuse'ами Electron: --inspect не работает
// (package.json → build.electronFuses). Харнессу нужен инспектор главного
// процесса, поэтому он запускает копию «<сборка>-qa»: ссылки на файлы
// сборки и копия исполняемого файла с включённым инспектором. Сама сборка
// не меняется — из неё собирается релиз.
async function qaExecutable(executable) {
  const { getCurrentFuseWire, flipFuses, FuseVersion, FuseV1Options } = require("@electron/fuses");
  const wire = await getCurrentFuseWire(executable);
  // 48 — FuseState.DISABLE (@electron/fuses/dist/constants).
  if (wire[FuseV1Options.EnableNodeCliInspectArguments] !== 48) return executable;

  const sourceDir = path.dirname(executable);
  const qaDir = `${sourceDir}-qa`;
  const target = path.join(qaDir, path.basename(executable));
  const stat = fs.statSync(executable);
  const stampFile = path.join(qaDir, ".qa-stamp.json");
  const stamp = JSON.stringify({ size: stat.size, mtimeMs: stat.mtimeMs });
  try {
    if (fs.readFileSync(stampFile, "utf8") === stamp && fs.existsSync(target)) return target;
  } catch (error) { /* копии ещё нет */ }

  fs.rmSync(qaDir, { recursive: true, force: true });
  fs.mkdirSync(qaDir, { recursive: true });
  for (const entry of fs.readdirSync(sourceDir, { withFileTypes: true })) {
    if (entry.name === path.basename(executable)) continue;
    const from = path.join(sourceDir, entry.name);
    const to = path.join(qaDir, entry.name);
    if (!IS_WINDOWS) fs.symlinkSync(from, to);
    else if (entry.isDirectory()) fs.symlinkSync(from, to, "junction");
    else fs.linkSync(from, to);
  }
  fs.copyFileSync(executable, target);
  await flipFuses(target, { version: FuseVersion.V1, [FuseV1Options.EnableNodeCliInspectArguments]: true });
  fs.writeFileSync(stampFile, stamp);
  return target;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

// Песочница одного прогона. seed задаёт стартовое состояние профиля:
// fresh — первый запуск без выбранной библиотеки, иначе библиотека уже выбрана.
function prepareSandbox(dir, seed = {}) {
  const layout = {
    root: dir,
    home: path.join(dir, "home"),
    appData: path.join(dir, "appdata"),
    localAppData: path.join(dir, "localappdata"),
    userData: path.join(dir, "appdata", "HLSR"),
    library: path.join(dir, "library", "HLSR"),
    temp: path.join(dir, "tmp"),
    pick: path.join(dir, "picked"),
    saves: path.join(dir, "saves"),
    events: path.join(dir, "events.jsonl"),
    log: path.join(dir, "launcher-stdout.log"),
  };
  for (const key of ["home", "appData", "localAppData", "userData", "temp", "pick", "saves"]) {
    fs.mkdirSync(layout[key], { recursive: true });
  }

  // Proton на Linux: без него лаунчер при старте скачивает сборку заново.
  // reflink-копия на btrfs мгновенна, не занимает места и изолирует оригинал.
  if (!IS_WINDOWS && seed.protonFrom && fs.existsSync(seed.protonFrom)) {
    const target = path.join(layout.userData, "protons", path.basename(seed.protonFrom));
    if (!fs.existsSync(target)) {
      fs.mkdirSync(path.dirname(target), { recursive: true });
      const copy = spawnSync("cp", ["-a", "--reflink=auto", seed.protonFrom, target], { stdio: "ignore" });
      if (copy.status !== 0) throw new Error(`Не удалось скопировать Proton: ${seed.protonFrom}`);
    }
  }

  // Установленные игры: копия настоящей библиотеки тестировщика.
  if (seed.libraryFrom && !seed.fresh) seedLibrary(seed.libraryFrom, layout.library);

  if (!seed.fresh) {
    fs.mkdirSync(layout.library, { recursive: true });
    writeJson(path.join(layout.userData, "library.json"), {
      libraryPath: layout.library,
      libraryConfirmed: true,
      ...(seed.library || {}),
    });
  }
  if (seed.language || seed.settings) {
    writeJson(path.join(layout.userData, "settings.json"), {
      config: { language: seed.language || null, ...((seed.settings && seed.settings.config) || {}) },
    });
  }
  return layout;
}

// Копия библиотеки для сценариев с установленными играми. На btrfs/XFS
// (Linux) это reflink: мгновенно и без расхода места, любые удаления и
// восстановления в песочнице не трогают оригинал. Абсолютные пути к
// оригиналу в installed.json и прочих служебных файлах переписываются на
// копию, затем проверяется, что ни одного не осталось: иначе «Удалить игру»
// в песочнице пошла бы по старому пути в настоящую библиотеку.
function seedLibrary(source, target) {
  const from = path.resolve(source);
  if (!fs.existsSync(from)) throw new Error(`Нет библиотеки для засева: ${from}`);
  if (fs.existsSync(target)) fs.rmSync(target, { recursive: true, force: true });
  fs.mkdirSync(path.dirname(target), { recursive: true });
  if (IS_WINDOWS) {
    fs.cpSync(from, target, { recursive: true });
  } else {
    const copy = spawnSync("cp", ["-a", "--reflink=auto", from, target], { stdio: "ignore" });
    if (copy.status !== 0) throw new Error(`Не удалось скопировать библиотеку: ${from}`);
  }

  const variants = (value) => Array.from(new Set([value, value.replace(/\\/g, "\\\\"), value.replace(/\\/g, "/")]));
  // Граница имени обязательна: /home/u/HLSR — префикс /home/u/HLSR-monolith,
  // такой путь не относится к библиотеке и меняться не должен.
  const escape = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const fromVariants = variants(from);
  const toVariants = variants(path.resolve(target));
  const matchers = fromVariants.map((old, index) => [
    new RegExp(`${escape(old)}(?![\\w.-])`, "g"),
    toVariants[Math.min(index, toVariants.length - 1)],
  ]);
  const leftovers = [];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
        continue;
      }
      if (!entry.isFile() || !/\.(json|cfg|txt|ini|vdf|log|xml|lss|lsl)$/i.test(entry.name)) continue;
      if (fs.statSync(full).size > 4 * 1024 * 1024) continue;
      const text = fs.readFileSync(full, "utf8");
      let next = text;
      for (const [pattern, fresh] of matchers) next = next.replace(pattern, () => fresh);
      if (next !== text) fs.writeFileSync(full, next);
      if (matchers.some(([pattern]) => new RegExp(pattern.source).test(next))) leftovers.push(full);
    }
  };
  walk(target);
  if (leftovers.length) throw new Error(`В копии библиотеки остались пути к оригиналу: ${leftovers.slice(0, 5).join(", ")}`);
}

function sandboxEnv(layout) {
  const env = { ...process.env };
  // Ключи моделей лаунчеру не нужны.
  for (const key of ["TYPESAFE_API_KEY", "CF_API_TOKEN", "CF_ACCOUNT_ID"]) delete env[key];
  delete env.ELECTRON_RUN_AS_NODE;
  delete env.APPIMAGE;
  delete env.APPDIR;

  delete env.NODE_OPTIONS;
  env.HLSR_QA_EVENTS = layout.events;
  env.HLSR_QA_USERDATA = layout.userData;
  env.HLSR_QA_PICK_DIR = layout.pick;
  env.HLSR_QA_SAVE_DIR = layout.saves;
  env.HLSR_QA_ROOT = layout.root;
  env.HLSR_QA_HOME = layout.home;
  env.HLSR_QA_TEMP = layout.temp;
  env.TMPDIR = layout.temp;
  env.TEMP = layout.temp;
  env.TMP = layout.temp;

  if (IS_WINDOWS) {
    env.USERPROFILE = layout.home;
    env.APPDATA = layout.appData;
    env.LOCALAPPDATA = layout.localAppData;
  } else {
    env.HOME = layout.home;
    env.XDG_CONFIG_HOME = layout.appData;
    env.XDG_DATA_HOME = path.join(layout.home, ".local", "share");
    env.XDG_STATE_HOME = path.join(layout.home, ".local", "state");
    env.XDG_CACHE_HOME = path.join(layout.home, ".cache");
  }
  return env;
}

function readEvents(layout) {
  try {
    return fs
      .readFileSync(layout.events, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  } catch (error) {
    return [];
  }
}

// Главный процесс стоит на первой строке (--inspect-brk): хук подгружается
// в его контексте до кода лаунчера, затем процесс отпускается.
async function installMainHook(inspectPort) {
  const target = await waitForTarget(inspectPort, (item) => item.type === "node", 60000);
  const main = await new CdpSession(target.webSocketDebuggerUrl).connect();
  const paused = new Promise((resolve) => main.on("Debugger.paused", resolve));
  await main.send("Runtime.enable");
  await main.send("Debugger.enable");
  await main.send("Runtime.runIfWaitingForDebugger");
  const { callFrames } = await paused;
  const hook = path.join(__dirname, "main-hook.js");
  const result = await main.send("Debugger.evaluateOnCallFrame", {
    callFrameId: callFrames[0].callFrameId,
    expression: `require(${JSON.stringify(hook)}); "ok"`,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    const details = result.exceptionDetails;
    throw new Error(`Хук не загрузился: ${(details.exception && details.exception.description) || details.text}`);
  }
  await main.send("Debugger.resume");
  await main.send("Debugger.disable");
  return main;
}

async function launch({ sandboxDir, seed = {}, executable = defaultExecutable(), width, height } = {}) {
  if (!fs.existsSync(executable)) {
    throw new Error(`Нет собранного лаунчера: ${executable}. Сначала npm run build:dir`);
  }
  executable = await qaExecutable(executable);
  const layout = prepareSandbox(sandboxDir, seed);
  const port = await freePort();
  const inspectPort = await freePort();
  const args = [
    `--inspect-brk=127.0.0.1:${inspectPort}`,
    `--remote-debugging-port=${port}`,
    "--remote-allow-origins=*",
  ];
  if (!IS_WINDOWS) args.unshift("--no-sandbox");

  const out = fs.openSync(layout.log, "a");
  const child = spawn(executable, args, {
    env: sandboxEnv(layout),
    cwd: layout.root,
    stdio: ["ignore", out, out],
    detached: !IS_WINDOWS,
  });
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve({ code, signal })));

  const failedEarly = exited.then(({ code, signal }) => {
    throw new Error(`Лаунчер завершился при старте (код ${code}, сигнал ${signal}). Лог: ${layout.log}`);
  });
  const main = await Promise.race([installMainHook(inspectPort), failedEarly]);

  const target = await Promise.race([
    waitForTarget(port, (item) => item.type === "page" && !/devtools:/.test(item.url), 90000),
    failedEarly,
  ]);
  const page = await new CdpSession(target.webSocketDebuggerUrl).connect();
  await page.send("Runtime.enable");
  await page.send("Page.enable");

  const consoleErrors = [];
  page.on("Runtime.exceptionThrown", (params) => {
    const details = params.exceptionDetails || {};
    consoleErrors.push({
      at: Date.now(),
      text: (details.exception && details.exception.description) || details.text || "exception",
    });
  });
  page.on("Runtime.consoleAPICalled", (params) => {
    if (params.type !== "error" && params.type !== "warning") return;
    const text = (params.args || []).map((arg) => arg.value || arg.description || "").join(" ");
    consoleErrors.push({ at: Date.now(), level: params.type, text });
  });

  if (width && height) {
    await page.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  }

  async function stop() {
    page.close();
    main.close();
    if (child.exitCode !== null) return;
    try {
      if (IS_WINDOWS) spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
      else process.kill(-child.pid, "SIGTERM");
    } catch (ignored) {
      // процесс уже завершился
    }
    await Promise.race([exited, delay(8000)]);
    if (child.exitCode === null && !IS_WINDOWS) {
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch (ignored) {
        // уже завершился
      }
    }
  }

  return {
    page,
    main,
    layout,
    port,
    child,
    exited,
    consoleErrors,
    events: () => readEvents(layout),
    stop,
  };
}

module.exports = { launch, prepareSandbox, seedLibrary, defaultExecutable, REPO };
