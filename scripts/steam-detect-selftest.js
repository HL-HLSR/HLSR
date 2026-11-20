"use strict";

// Определение запущенного Steam на Windows, когда WMI или PowerShell не
// отвечают: PowerShell берётся по полному пути, без него — tasklist, и Steam
// из реестра принимается по имени образа.
const test = require("node:test");
const assert = require("node:assert/strict");

const SteamLocator = require("../src/main/game-delivery/SteamLocator");
const processInspect = require("../src/main/game-delivery/process-inspect");
const { createWindowsInspector, parseTasklist } = processInspect;

const STEAM = { installPath: "C:\\Program Files (x86)\\Steam", steamExe: "C:\\Program Files (x86)\\Steam\\steam.exe" };
const ENV = { SystemRoot: "C:\\Windows" };

function windowsInspector(handlers) {
  const calls = [];
  const inspector = createWindowsInspector({
    env: ENV,
    existsSync: () => true,
    execFileSync(file, args) {
      calls.push(file);
      const tool = /powershell/i.test(file) ? "powershell" : "tasklist";
      const handler = handlers[tool];
      if (!handler) throw new Error(`${tool} не задан`);
      return handler(args);
    },
  });
  return { inspector, calls };
}

function locator(inspector, logger) {
  return new SteamLocator({
    platform: "win32",
    registry: { read: (key, name) => (name === "pid" ? 7440 : null) },
    inspector: Object.assign({}, processInspect, { isAlive: () => true }, inspector),
    logger: logger || { info() {}, warn() {}, error() {} },
  });
}

test("tasklist: разбор CSV, строка «нет задач» пропускается", () => {
  const output = '"steam.exe","7440","Console","1","123 456 K"\r\n"steam.exe","812","Console","1","9 000 K"\r\n';
  assert.deepEqual(parseTasklist(output).map((item) => [item.name, item.pid, item.source]), [
    ["steam.exe", 7440, "tasklist"],
    ["steam.exe", 812, "tasklist"],
  ]);
  assert.deepEqual(parseTasklist("ИНФОРМАЦИЯ: Задачи, отвечающие заданным критериям, отсутствуют.\r\n"), []);
});

test("PowerShell берётся по полному пути из SystemRoot", () => {
  const { inspector, calls } = windowsInspector({
    powershell: () => JSON.stringify({ pid: 7440, executablePath: STEAM.steamExe, name: "steam.exe", source: "cim" }),
  });
  const actual = inspector.inspect(7440);
  assert.equal(actual.executablePath, STEAM.steamExe);
  assert.equal(calls[0], "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
});

test("PowerShell не запускается — процесс опознаётся через tasklist, причина сохраняется", () => {
  const { inspector, calls } = windowsInspector({
    powershell: () => {
      throw Object.assign(new Error("spawn powershell.exe ENOENT"), { code: "ENOENT" });
    },
    tasklist: (args) => {
      assert.deepEqual(args.slice(-2), ["/FI", "PID eq 7440"]);
      return '"steam.exe","7440","Console","1","123 456 K"\r\n';
    },
  });
  const actual = inspector.inspect(7440);
  assert.deepEqual([actual.name, actual.executablePath, actual.source], ["steam.exe", null, "tasklist"]);
  assert.equal(calls[1], "C:\\Windows\\System32\\tasklist.exe");
  assert.match(inspector.lastFailure(), /ENOENT/);
});

test("Steam из реестра принимается по имени, если путь узнать нечем", () => {
  const warnings = [];
  const { inspector } = windowsInspector({
    powershell: () => {
      throw new Error("Доступ запрещён");
    },
    tasklist: () => '"steam.exe","7440","Console","1","123 456 K"\r\n',
  });
  const running = locator(inspector, { info() {}, warn: (text) => warnings.push(text), error() {} }).assertSteamRunning(STEAM);
  assert.equal(running.pid, 7440);
  assert.equal(running.source, "registry-name");
  assert.match(warnings.join("\n"), /Доступ запрещён/);
});

test("чужой процесс под PID из реестра Steam не считается", () => {
  const { inspector } = windowsInspector({
    powershell: () => {
      throw new Error("WMI недоступен");
    },
    tasklist: (args) => (args.includes("PID eq 7440") ? '"chrome.exe","7440","Console","1","1 K"\r\n' : ""),
  });
  assert.throws(() => locator(inspector).assertSteamRunning(STEAM), (error) => {
    assert.equal(error.code, "STEAM_NOT_RUNNING");
    assert.match(error.message, /WMI недоступен/);
    return true;
  });
});

test("процессы steam.exe от tasklist без пути — не «Steam с другими правами»", () => {
  const { inspector } = windowsInspector({
    powershell: () => {
      throw new Error("WMI недоступен");
    },
    tasklist: (args) => (args.includes("IMAGENAME eq steam.exe") ? '"steam.exe","812","Console","1","1 K"\r\n' : ""),
  });
  assert.throws(() => locator(inspector).assertSteamRunning(STEAM), { code: "STEAM_NOT_RUNNING" });
});

test("путь от PowerShell по-прежнему сверяется с путём Steam", () => {
  const { inspector } = windowsInspector({
    powershell: () => JSON.stringify({ pid: 7440, executablePath: "D:\\Other\\steam.exe", name: "steam.exe", source: "process" }),
  });
  assert.throws(() => locator(inspector).assertSteamRunning(STEAM), (error) => {
    assert.equal(error.code, "STEAM_NOT_RUNNING");
    assert.match(error.message, /не Steam/);
    return true;
  });
});

test("powershell.exe везде вызывается по полному пути, а не по имени из PATH", () => {
  const { powershellPath } = require("../src/main/windows-tools");
  assert.equal(powershellPath({ SystemRoot: "D:\\Win" }), "D:\\Win\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
  assert.equal(powershellPath({ WINDIR: "C:\\WINDOWS" }), "C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");

  const fs = require("fs");
  const path = require("path");
  const offenders = [];
  (function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (/\.(js|vue)$/.test(entry.name) && /\b(spawn|spawnSync|execFile|execFileSync|spawnFn|run|exec)\(\s*["'`]powershell(\.exe)?["'`]/i.test(fs.readFileSync(full, "utf8")))
        offenders.push(path.relative(path.join(__dirname, ".."), full));
    }
  })(path.join(__dirname, "..", "src"));
  assert.deepEqual(offenders, []);
});

test("ссылка на каталог депота ставится без cmd.exe: «&» и пробел в пути библиотеки не запускают команду", async () => {
  const fs = require("fs");
  const os = require("os");
  const path = require("path");
  const SteamDepotService = require("../src/main/game-delivery/steam-depot");

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-depot-"));
  try {
    const root = path.join(temp, "R&D lib&echo.HLSR_INJECTED&");
    const target = path.join(root, ".hlsr", "staging", "hl2-5288-depot-x");
    const slot = path.join(temp, "content", "app_2");
    fs.mkdirSync(path.dirname(slot), { recursive: true });

    const runs = [];
    const service = new SteamDepotService({ run: async (...args) => { runs.push(args); return { status: 0 }; } });
    assert.equal(await service.linkContentSlot(slot, target, root), slot);

    assert.deepEqual(runs, []);
    assert.ok(fs.lstatSync(slot).isSymbolicLink());
    assert.equal(path.resolve(fs.readlinkSync(slot)), target);
    assert.equal(service.unlinkContentSlot(slot), true);
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
});

test("main и апдейтер не отдают строки команд оболочке: нет exec/execSync, shell: true, cmd.exe и ComSpec", () => {
  const fs = require("fs");
  const path = require("path");
  const CHILD_PROCESS = String.raw`(?:require\(\s*["']child_process["']\s*\)|from\s+["']child_process["'])`;
  const usesShell = (text) => {
    // Имена, под которыми подключён child_process: const cp = require(...), import cp / * as cp from.
    const aliases = [...text.matchAll(new RegExp(String.raw`(?:const|let|var|import)\s+(?:\*\s+as\s+)?(\w+)\s*=?\s*${CHILD_PROCESS}`, "g"))]
      .map((match) => match[1]);
    return new RegExp(String.raw`\{[^}]*\b(exec|execSync)\b[^}]*\}\s*=?\s*${CHILD_PROCESS}`).test(text) ||
      new RegExp(String.raw`${CHILD_PROCESS}\s*\.\s*(exec|execSync)\b`).test(text) ||
      aliases.some((name) => new RegExp(String.raw`\b${name}\s*\.\s*(exec|execSync)\s*\(`).test(text)) ||
      /shell:\s*true/.test(text) || /["'`]cmd(\.exe)?["'`]/i.test(text) || /\bcomspec\b/i.test(text);
  };

  // Шаблон ловит все известные формы и не путает их с RegExp.exec и execFile.
  for (const sample of ['const { execSync } = require("child_process");', 'import { exec } from "child_process";',
    'require("child_process").execSync(`fsutil volume diskfree ${drive}`);', 'const cp = require("child_process"); cp.exec(line);',
    'import * as cp from "child_process"; cp.execSync(line);', 'spawn(process.env.ComSpec, ["/c", "mklink", slot, target]);',
    'execFile("cmd.exe", ["/c", "mklink"]);', 'spawn(path.join(root, "System32", "cmd.exe"));', 'spawn(file, args, { shell: true });'])
    assert.equal(usesShell(sample), true, sample);
  for (const sample of ['const match = /^(\\d+)$/.exec(line);', 'RUN_PAGE.exec(link);', 'const childProcess = require("child_process"); childProcess.execFile("reg", ["query"]);',
    'const { execFileSync } = require("child_process"); execFileSync("fsutil", ["volume", "diskfree", root]);', '// раньше здесь был cmd.exe /c mklink'])
    assert.equal(usesShell(sample), false, sample);

  const offenders = [];
  (function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (/\.js$/.test(entry.name) && usesShell(fs.readFileSync(full, "utf8")))
        offenders.push(path.relative(path.join(__dirname, ".."), full));
    }
  })(path.join(__dirname, "..", "src"));
  assert.deepEqual(offenders, []);
});
