#!/usr/bin/env node
"use strict";

// Компиляция main-бандла в байткод V8 (bytenode).
//
// После webpack в dist/electron лежит main.js со всем кодом основного
// процесса. Этот шаг компилирует его в main.jsc, проверяет, что байткод
// принимает настоящий main-процесс Electron, и заменяет main.js загрузчиком.
// package.json по-прежнему указывает на dist/electron/main.js.
//
// Байткод привязан к версии Electron, платформе и типу процесса: на Electron
// 42+ его надо компилировать внутри main-процесса того же Electron, который
// будет его исполнять (bytenode: compileElectronMainCode). Windows-сборку на
// Linux компилирует Windows-Electron под Wine (scripts/lib/wine-electron.js).
// Байткод затрудняет чтение кода и ключа аттестации,
// но не защищает от декомпиляции: строки в байткоде лежат как есть.
//
// Нетронутый бандл сохраняется в release/launcher-symbols/<платформа>-<версия>/:
// в стеках из отчётов о сбоях позиции указывают в main.jsc, а имена функций
// остаются, и по сохранённому бандлу позицию можно сопоставить с кодом.
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn, spawnSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const DIST = path.join(ROOT, "dist", "electron");
const BUNDLE = "main.js";
const BYTECODE = "main.jsc";
const LOADER_MARK = "// hlsr-bytecode-loader";

// Загрузчик не содержит секретов и выполняется до всего остального кода.
const LOADER = `"use strict";
${LOADER_MARK}
// Код основного процесса собран в main.jsc (scripts/compile-main-bytecode.js).
try {
  require("bytenode");
  require("./main.jsc");
} catch (error) {
  const { app, dialog } = require("electron");
  process.stderr.write("HLSR: не удалось запустить лаунчер: " + (error && error.stack || error) + "\\n");
  try { dialog.showErrorBox("HLSR", "Не удалось запустить лаунчер.\\n" + (error && error.message || error)); } catch (ignored) { /* окна нет */ }
  app.exit(1);
}
`;

function loaderSource() {
  return LOADER;
}

function isLoader(text) {
  return text.indexOf(LOADER_MARK) !== -1 && text.length < 4096;
}

function packageVersion() {
  return JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8")).version;
}

// Проверка в настоящем main-процессе: байткод принят и разворачивается в
// функцию-обёртку модуля. Само приложение при этом не запускается.
function verify(bytecodeFile, electronPath) {
  return new Promise((resolve, reject) => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-bytecode-verify-"));
    const script = path.join(dir, "verify.js");
    fs.writeFileSync(script, [
      "const { app } = require('electron');",
      `const bytenode = require(${JSON.stringify(require.resolve("bytenode"))});`,
      "app.whenReady().then(() => {",
      "  try {",
      `    const wrapper = bytenode.runBytecodeFile(${JSON.stringify(bytecodeFile)});`,
      "    app.exit(typeof wrapper === 'function' ? 0 : 3);",
      "  } catch (error) {",
      "    process.stderr.write(String((error && error.stack) || error) + '\\n');",
      "    app.exit(1);",
      "  }",
      "});",
    ].join("\n"));
    const child = spawn(electronPath, ["--no-sandbox", script], { stdio: ["ignore", "pipe", "pipe"] });
    let output = "";
    child.stdout.on("data", (chunk) => { output += chunk; });
    child.stderr.on("data", (chunk) => { output += chunk; });
    child.on("error", (error) => { fs.rmSync(dir, { recursive: true, force: true }); reject(error); });
    child.on("close", (code, signal) => {
      fs.rmSync(dir, { recursive: true, force: true });
      if (code === 0) return resolve();
      const how = signal ? `сигнал ${signal}` : `код ${code}`;
      reject(new Error(`Electron не принял байткод (${how}):\n${output.trim().split("\n").slice(-12).join("\n")}`));
    });
  });
}

async function compileMain(options = {}) {
  const dist = path.resolve(options.dist || DIST);
  const source = path.join(dist, BUNDLE);
  const target = path.join(dist, BYTECODE);
  const buildTarget = require("./lib/build-target");
  const platform = buildTarget.targetPlatform();
  // Windows-сборка на Linux: байткод компилирует Windows-Electron под Wine.
  const electronPath = options.electronPath ||
    (buildTarget.isCross() && platform === "win32" ? await require("./lib/wine-electron").electronPath() : require("electron"));
  if (buildTarget.isCross() && platform !== "win32" && !options.electronPath)
    throw new Error(`Байткод для ${platform} на ${process.platform} не собрать: соберите на ${platform}`);
  if (!fs.existsSync(source)) throw new Error(`Нет ${source}: сначала соберите main (npm run pack:main)`);
  const text = fs.readFileSync(source, "utf8");
  if (isLoader(text)) throw new Error(`${source} уже загрузчик байткода: пересоберите бандл перед повторной компиляцией`);

  const symbols = path.join(options.symbols || path.join(ROOT, "release", "launcher-symbols"), `${platform}-${packageVersion()}`);
  fs.mkdirSync(symbols, { recursive: true });
  fs.copyFileSync(source, path.join(symbols, BUNDLE));

  const bytenode = require("bytenode");
  await bytenode.compileFile({ filename: source, output: target, electronMain: true, electronPath });
  await verify(target, electronPath);
  fs.writeFileSync(source, LOADER);
  return { source: text.length, bytecode: fs.statSync(target).size, symbols };
}

// На Linux Electron для компиляции нужен дисплей. Нет ни его, ни Wayland —
// запускаемся заново (тот же скрипт с теми же аргументами) под xvfb-run,
// если он установлен.
function ensureDisplay() {
  if (process.platform !== "linux" || process.env.DISPLAY || process.env.WAYLAND_DISPLAY || process.env.HLSR_XVFB === "1") return false;
  const probe = spawnSync("xvfb-run", ["--help"], { stdio: "ignore" });
  if (probe.error) throw new Error("Для компиляции байткода Electron нужен дисплей: установите xvfb или запустите сборку в графическом сеансе");
  const result = spawnSync("xvfb-run", ["-a", process.execPath, process.argv[1], ...process.argv.slice(2)],
    { stdio: "inherit", env: Object.assign({}, process.env, { HLSR_XVFB: "1" }) });
  process.exit(result.status === null ? 1 : result.status);
}

if (require.main === module) {
  ensureDisplay();
  compileMain().then((result) => {
    process.stdout.write(`Байткод main: ${(result.source / 1048576).toFixed(1)} МБ кода → ${(result.bytecode / 1048576).toFixed(1)} МБ main.jsc; ` +
      `исходный бандл сохранён в ${path.relative(ROOT, result.symbols)}\n`);
  }, (error) => {
    process.stderr.write(`Компиляция байткода не удалась: ${error.message}\n`);
    process.exit(1);
  });
}

module.exports = { compileMain, ensureDisplay, loaderSource, isLoader, verify, BUNDLE, BYTECODE };
