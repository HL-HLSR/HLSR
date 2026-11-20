"use strict";

// Хук главного процесса тестового лаунчера. Драйвер подгружает его через
// инспектор (--inspect-brk) до первой строки кода приложения. Задача —
// удержать тестовый экземпляр в песочнице и записать всё, что он пытался
// сделать снаружи:
// - пути ОС (userData, home, рабочий стол, temp…) переносятся в песочницу;
//   на Windows appData берётся из известных папок системы, переменные
//   окружения его не меняют;
// - shell.* и dialog.* не открывают окна на рабочем столе тестировщика:
//   вызов пишется в журнал, выбор папки возвращает папку песочницы;
// - запись, удаление и переименование файлов вне песочницы запрещены
//   (EACCES) и попадают в журнал как находка;
// - внешние процессы: архиваторы и чтение списка процессов разрешены, всё
//   остальное (игры, Proton/umu, Steam, LiveSplit, pkexec, xdg-open…)
//   заменяется процессом-заглушкой, который живёт HLSR_QA_GAME_SECONDS.
// Модули лаунчера берут spawn/rmSync деструктуризацией при загрузке, поэтому
// подмена до старта приложения действует на все вызовы.

if (process.type === "browser" && process.env.HLSR_QA_EVENTS && !global.__hlsrQaHook) {
  global.__hlsrQaHook = true;

  const fs = require("fs");
  const path = require("path");
  const childProcess = require("child_process");
  const electron = require("electron");

  const IS_WINDOWS = process.platform === "win32";
  const eventsFile = process.env.HLSR_QA_EVENTS;
  const sandboxRoot = path.resolve(process.env.HLSR_QA_ROOT || path.dirname(eventsFile));
  const gameSeconds = Number(process.env.HLSR_QA_GAME_SECONDS || 20);

  const original = {
    appendFileSync: fs.appendFileSync,
    spawn: childProcess.spawn,
    execFile: childProcess.execFile,
  };

  const record = (kind, payload) => {
    try {
      original.appendFileSync(eventsFile, `${JSON.stringify({ at: Date.now(), kind, ...payload })}\n`);
    } catch (ignored) {
      // журнал вспомогательный
    }
  };

  // --- пути ОС --------------------------------------------------------------
  const { app } = electron;
  if (process.env.HLSR_QA_USERDATA) app.setPath("userData", process.env.HLSR_QA_USERDATA);
  const home = process.env.HLSR_QA_HOME;
  if (home) {
    for (const [name, sub] of [
      ["home", ""],
      ["desktop", "Desktop"],
      ["documents", "Documents"],
      ["downloads", "Downloads"],
      ["pictures", "Pictures"],
      ["music", "Music"],
      ["videos", "Videos"],
    ]) {
      const target = path.join(home, sub);
      fs.mkdirSync(target, { recursive: true });
      app.setPath(name, target);
    }
  }
  if (process.env.HLSR_QA_TEMP) app.setPath("temp", process.env.HLSR_QA_TEMP);

  // --- shell и диалоги --------------------------------------------------------
  const { shell, dialog } = electron;
  shell.openExternal = async (url) => record("openExternal", { url: String(url) });
  shell.openPath = async (target) => {
    record("openPath", { path: String(target), exists: fs.existsSync(String(target)) });
    return "";
  };
  shell.showItemInFolder = (target) => record("showItemInFolder", { path: String(target), exists: fs.existsSync(String(target)) });
  shell.trashItem = async (target) => {
    record("trashItem", { path: String(target) });
    throw new Error("trashItem отключён в тестовом прогоне");
  };
  shell.beep = () => record("beep", {});

  const optionsOf = (args) => args.find((item) => item && typeof item === "object" && !item.webContents) || {};
  const pickDir = process.env.HLSR_QA_PICK_DIR || "";
  const pickFile = process.env.HLSR_QA_PICK_FILE || "";
  const openDialog = (args) => {
    const options = optionsOf(args);
    const wantsDir = Array.isArray(options.properties) && options.properties.includes("openDirectory");
    const picked = wantsDir ? pickDir : pickFile;
    record("showOpenDialog", { title: options.title || "", properties: options.properties || [], picked });
    return picked ? { canceled: false, filePaths: [picked] } : { canceled: true, filePaths: [] };
  };
  dialog.showOpenDialog = async (...args) => openDialog(args);
  dialog.showOpenDialogSync = (...args) => {
    const result = openDialog(args);
    return result.canceled ? undefined : result.filePaths;
  };
  dialog.showSaveDialog = async (...args) => {
    const options = optionsOf(args);
    const target = path.join(process.env.HLSR_QA_SAVE_DIR || pickDir, path.basename(options.defaultPath || "export.bin"));
    record("showSaveDialog", { title: options.title || "", target });
    return { canceled: false, filePath: target };
  };
  dialog.showSaveDialogSync = (...args) => {
    const options = optionsOf(args);
    const target = path.join(process.env.HLSR_QA_SAVE_DIR || pickDir, path.basename(options.defaultPath || "export.bin"));
    record("showSaveDialog", { title: options.title || "", target });
    return target;
  };
  dialog.showMessageBox = async (...args) => {
    const options = optionsOf(args);
    record("showMessageBox", { message: options.message || "", detail: options.detail || "", buttons: options.buttons || [] });
    return { response: typeof options.cancelId === "number" ? options.cancelId : 0, checkboxChecked: false };
  };
  dialog.showMessageBoxSync = (...args) => {
    const options = optionsOf(args);
    record("showMessageBox", { message: options.message || "", buttons: options.buttons || [] });
    return typeof options.cancelId === "number" ? options.cancelId : 0;
  };
  dialog.showErrorBox = (title, content) => record("showErrorBox", { title: String(title), content: String(content) });

  // --- файловая система -------------------------------------------------------
  const allowedRoots = [sandboxRoot, app.getPath("userData")].map((item) => path.resolve(item));
  const inSandbox = (target) => {
    if (target === undefined || target === null) return true;
    if (typeof target === "number") return true; // файловый дескриптор
    let value = target;
    if (value instanceof URL) value = value.pathname;
    if (Buffer.isBuffer(value)) value = value.toString();
    value = String(value);
    if (!path.isAbsolute(value)) value = path.resolve(value);
    const resolved = path.resolve(value);
    const compare = IS_WINDOWS ? (item) => item.toLowerCase() : (item) => item;
    return allowedRoots.some((root) => {
      const a = compare(resolved);
      const b = compare(root);
      return a === b || a.startsWith(b + path.sep);
    });
  };
  const deny = (op, target) => {
    record("fs-blocked", { op, path: String(target) });
    const error = new Error(`EACCES: тестовый прогон не пишет вне песочницы: ${op} '${target}'`);
    error.code = "EACCES";
    error.path = String(target);
    return error;
  };

  // имя операции → индексы аргументов-путей, которые она меняет
  const MUTATING = {
    writeFile: [0],
    appendFile: [0],
    rm: [0],
    rmdir: [0],
    unlink: [0],
    rename: [0, 1],
    copyFile: [1],
    cp: [1],
    mkdir: [0],
    symlink: [1],
    link: [1],
    truncate: [0],
    chmod: [0],
    utimes: [0],
    mkdtemp: [0],
  };
  for (const [op, positions] of Object.entries(MUTATING)) {
    for (const variant of [op, `${op}Sync`]) {
      const fn = fs[variant];
      if (typeof fn !== "function") continue;
      const sync = variant.endsWith("Sync");
      fs[variant] = function guarded(...args) {
        const bad = positions.map((index) => args[index]).find((target) => !inSandbox(target));
        if (bad !== undefined) {
          const error = deny(variant, bad);
          if (sync) throw error;
          const callback = args[args.length - 1];
          if (typeof callback === "function") return process.nextTick(callback, error);
          throw error;
        }
        return fn.apply(this, args);
      };
    }
    const promised = fs.promises[op];
    if (typeof promised === "function") {
      fs.promises[op] = async function guarded(...args) {
        const bad = positions.map((index) => args[index]).find((target) => !inSandbox(target));
        if (bad !== undefined) throw deny(`promises.${op}`, bad);
        return promised.apply(this, args);
      };
    }
  }
  const createWriteStream = fs.createWriteStream;
  fs.createWriteStream = function guarded(target, ...rest) {
    if (!inSandbox(target)) throw deny("createWriteStream", target);
    return createWriteStream.call(this, target, ...rest);
  };
  const open = fs.open;
  const openSync = fs.openSync;
  const writes = (flags) => typeof flags === "string" && /[wa+]/.test(flags);
  fs.open = function guarded(target, flags, ...rest) {
    if (writes(flags) && !inSandbox(target)) {
      const callback = rest[rest.length - 1];
      const error = deny("open", target);
      if (typeof callback === "function") return process.nextTick(callback, error);
      throw error;
    }
    return open.call(this, target, flags, ...rest);
  };
  fs.openSync = function guarded(target, flags, ...rest) {
    if (writes(flags) && !inSandbox(target)) throw deny("openSync", target);
    return openSync.call(this, target, flags, ...rest);
  };

  // --- внешние процессы -------------------------------------------------------
  const base = (file) => path.basename(String(file || "")).toLowerCase().replace(/\.exe$/, "");
  const ALLOWED = new Set(["7z", "7za", "7zz", "7zr", "tar", "ps", "pgrep", "df", "tasklist", "wmic", "uname", "lsb_release", "getconf", "nproc"]);
  const allowCall = (file, args) => {
    const name = base(file);
    if (ALLOWED.has(name)) return true;
    // reg.exe только на чтение: SteamLocator ищет путь Steam
    if (name === "reg") return String((args || [])[0] || "").toLowerCase() === "query";
    // powershell/cmd лаунчер зовёт для свободного места и списка процессов;
    // команды с Remove/Stop/Start/Set остаются заблокированными
    if (name === "powershell" || name === "pwsh") {
      const text = (args || []).join(" ");
      return /Get-|Win32_LogicalDisk|Win32_Process/i.test(text) && !/Remove-|Stop-|Start-|Set-|New-|Invoke-WebRequest/i.test(text);
    }
    return false;
  };
  const dummyArgs = (seconds) => ["-e", `setTimeout(() => {}, ${Math.max(0, seconds) * 1000})`];
  const dummyEnv = (options) => ({ ...((options && options.env) || process.env), ELECTRON_RUN_AS_NODE: "1" });
  const isGameLike = (file, args) =>
    /umu|proton|wine|hl2|hl\.exe|^hl$|livesplit|steam|bxt|ghosting|game/i.test(`${base(file)} ${(args || []).join(" ")}`);
  const argsOf = (args) => (Array.isArray(args) ? args.map(String) : []);
  const optionsAfter = (args) => (Array.isArray(args[1]) ? args[2] : args[1]) || {};

  const ownPids = new Set();
  childProcess.spawn = function guardedSpawn(file, args, options) {
    const list = argsOf(args);
    if (allowCall(file, list)) return original.spawn.apply(this, arguments);
    const opts = Array.isArray(args) ? options || {} : args || {};
    const seconds = isGameLike(file, list) ? gameSeconds : 0;
    record("spawn-stubbed", { file: String(file), args: list.slice(0, 20), cwd: opts.cwd || "", seconds });
    const child = original.spawn.call(this, process.execPath, dummyArgs(seconds), {
      ...opts,
      shell: false,
      env: dummyEnv(opts),
      cwd: opts.cwd && fs.existsSync(opts.cwd) ? opts.cwd : sandboxRoot,
    });
    if (child.pid) ownPids.add(child.pid);
    return child;
  };
  childProcess.execFile = function guardedExecFile(file, ...rest) {
    const list = argsOf(rest[0]);
    if (allowCall(file, list)) return original.execFile.call(this, file, ...rest);
    const callback = rest.find((item) => typeof item === "function");
    const opts = optionsAfter(rest.filter((item) => typeof item !== "function").length ? [file, ...rest] : [file]);
    record("execFile-stubbed", { file: String(file), args: list.slice(0, 20) });
    return original.execFile.call(this, process.execPath, dummyArgs(0), { env: dummyEnv(opts) }, (error) => {
      if (callback) callback(error || null, "", "");
    });
  };
  const execFileSync = childProcess.execFileSync;
  childProcess.execFileSync = function guardedExecFileSync(file, args, options) {
    const list = argsOf(args);
    if (allowCall(file, list)) return execFileSync.apply(this, arguments);
    record("execFileSync-stubbed", { file: String(file), args: list.slice(0, 20) });
    const opts = Array.isArray(args) ? options : args;
    return opts && opts.encoding && opts.encoding !== "buffer" ? "" : Buffer.alloc(0);
  };
  const spawnSync = childProcess.spawnSync;
  childProcess.spawnSync = function guardedSpawnSync(file, args, options) {
    const list = argsOf(args);
    if (allowCall(file, list)) return spawnSync.apply(this, arguments);
    record("spawnSync-stubbed", { file: String(file), args: list.slice(0, 20) });
    const opts = Array.isArray(args) ? options : args;
    const empty = opts && opts.encoding && opts.encoding !== "buffer" ? "" : Buffer.alloc(0);
    return { pid: 0, status: 0, signal: null, output: [null, empty, empty], stdout: empty, stderr: empty };
  };
  for (const name of ["exec", "execSync", "fork"]) {
    const fn = childProcess[name];
    childProcess[name] = function guardedShell(command, ...rest) {
      record(`${name}-stubbed`, { command: String(command).slice(0, 300) });
      if (name === "execSync") return Buffer.alloc(0);
      const callback = rest.find((item) => typeof item === "function");
      return original.execFile.call(this, process.execPath, dummyArgs(0), { env: dummyEnv() }, (error) => {
        if (callback) callback(error || null, "", "");
      });
    };
    childProcess[name].__original = fn;
  }

  // Лаунчер ищет игру по списку процессов и может попытаться закрыть её.
  // Сигнал проходит только к заглушкам этого экземпляра: настоящая игра,
  // запущенная тестировщиком параллельно, не пострадает.
  const kill = process.kill;
  process.kill = function guardedKill(pid, signal) {
    const target = Math.abs(Number(pid));
    if (target === process.pid || signal === 0 || ownPids.has(target)) return kill.apply(this, arguments);
    record("process-kill-blocked", { pid: Number(pid), signal: signal === undefined ? "SIGTERM" : signal });
    return true;
  };

  process.on("exit", (code) => record("main-exit", { code }));
  record("hook-ready", { userData: app.getPath("userData"), sandboxRoot, pid: process.pid });
}
