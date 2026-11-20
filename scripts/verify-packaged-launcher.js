#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const WIN_UNPACKED = path.join(ROOT, "build", "win-unpacked");

const problems = [];
const notes = [];

function fail(message) {
  problems.push(message);
}

function note(message) {
  notes.push(message);
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

const REQUIRED_MARKERS = [
  ["колонка иконок игр (редизайн)", "L_RAIL_ONLINE"],
  ["реестр доступности game-availability", "resolveStartupGame"],
  ["GameSettingsModal", "game-settings-get"],
  ["Notification Center", "notifications-list"],
  ["boot-ворота обновления", "boot-gate-run"],
  ["Steam-индикатор", "steam-health-get"],
  ["вкладка Диагностика", "diagnostics-run"],
  ["честная статистика", "game-stats-get"],
  ["расшифровка недоступности", "L_UA_TITLE"],
  ["контракт метаданных сборки", "ghostingmod/hlsr-build.json"],
  ["reason коды метаданных", "BUILD_METADATA_MISMATCH"],
  ["локализация метаданных сборки", "L_UA_BUILD_NEWER_LAUNCHER"],
  ["диагностика метаданных сборки", "build-meta-trust"],
];

const FORBIDDEN_MARKERS = [
  ["демо-дата обновления сборки", "09.04.2025"],
  ["sample build metadata", "ghosting-renewed-sample-0000000"],
  // С 0.4.8 строки авторов на экране загрузки нет (решение владельца).
  ["строка авторов на экране загрузки", "Developed by InfiniteC0re"],
];

function newestSourceMtime() {
  let newest = 0;
  let newestFile = null;

  const walk = (dir) => {
    for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, item.name);
      if (item.isDirectory()) walk(full);
      else {
        const mtime = fs.statSync(full).mtimeMs;
        if (mtime > newest) {
          newest = mtime;
          newestFile = full;
        }
      }
    }
  };

  walk(path.join(ROOT, "src"));
  return { newest, newestFile };
}

// Версию берём из корневого package.json внутри asar. Поиск первого
// "version" по сырым байтам ломается от порядка файлов: electron-builder 26
// кладёт node_modules (например, fontawesome) раньше package.json лаунчера.
function asarPackageVersion(asarPath) {
  try {
    const text = require("@electron/asar").extractFile(asarPath, "package.json").toString("utf8");
    const version = JSON.parse(text).version;
    return typeof version === "string" ? version : null;
  } catch (error) {
    return null;
  }
}

function checkAsarContent(asarPath, label) {
  const buffer = fs.readFileSync(asarPath);
  const text = buffer.toString("latin1");

  for (const [what, marker] of REQUIRED_MARKERS)
    if (text.indexOf(marker) === -1)
      fail(`${label}: нет маркера «${what}» (${marker}) — bundle старый`);

  for (const [what, marker] of FORBIDDEN_MARKERS)
    if (text.indexOf(marker) !== -1)
      fail(`${label}: найден устаревший маркер «${what}» (${marker})`);

  const pkg = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8"));

  const version = asarPackageVersion(asarPath);
  if (!version) fail(`${label}: не найдена версия внутри asar`);
  else if (version !== pkg.version)
    fail(`${label}: версия в asar ${version} != package.json ${pkg.version}`);

  return { text, version };
}

function checkWinUnpacked() {
  const pkg = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8"));
  const asarPath = path.join(WIN_UNPACKED, "resources", "app.asar");

  if (!fs.existsSync(asarPath)) {
    fail(`нет ${asarPath} — сборка не выполнялась`);
    return null;
  }

  checkAsarContent(asarPath, "win-unpacked app.asar");

  const { newest, newestFile } = newestSourceMtime();
  const asarMtime = fs.statSync(asarPath).mtimeMs;

  if (asarMtime < newest)
    fail(
      `app.asar (${new Date(asarMtime).toISOString()}) старше исходника ` +
        `${path.relative(ROOT, newestFile)} (${new Date(newest).toISOString()}) — пересоберите`
    );

  const mainNode = path.join(
    WIN_UNPACKED,
    "resources",
    "app.asar.unpacked",
    "node_modules",
    "hlsr-native",
    "build",
    "Release",
    "main.node"
  );
  if (!fs.existsSync(mainNode)) fail(`нет нативного модуля: ${mainNode}`);

  const latestYml = path.join(ROOT, "build", "latest.yml");
  if (!fs.existsSync(latestYml)) fail("нет build/latest.yml");
  else {
    const yml = fs.readFileSync(latestYml, "utf8");
    if (yml.indexOf(`version: ${pkg.version}`) === -1)
      fail(`latest.yml не содержит version: ${pkg.version}`);

    const wrong = yml.match(/version:\s*(\d+\.\d+\.\d+)/);
    if (wrong && wrong[1] !== pkg.version)
      fail(`latest.yml описывает версию ${wrong[1]} вместо ${pkg.version}`);
  }

  note(`win-unpacked app.asar: OK (${sha256(asarPath).slice(0, 16)}…)`);
  return { asarPath, asarSha: sha256(asarPath) };
}

function checkSetup(setupPath, unpackedInfo) {
  const pkg = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8"));

  if (!fs.existsSync(setupPath)) {
    fail(`нет установщика: ${setupPath}`);
    return;
  }

  const name = path.basename(setupPath);
  if (name !== `HLSR-Setup-${pkg.version}.exe`)
    fail(`имя установщика ${name} не соответствует версии ${pkg.version}`);

  // Нужен полный 7-Zip: 7za из 7zip-bin не открывает exe NSIS.
  const candidates = [
    "C:\\Program Files\\7-Zip\\7z.exe",
    "/usr/bin/7z",
    "/usr/bin/7zz",
    path.join(ROOT, "node_modules", "7zip-bin", "win", "x64", "7za.exe"),
  ];
  const sevenZip = candidates.find((p) => fs.existsSync(p));
  if (!sevenZip) {
    fail("7z.exe/7za.exe не найдены — распаковка Setup невозможна");
    return;
  }

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-verify-"));

  try {
    let result = spawnSync(sevenZip, ["x", "-y", `-o${temp}`, setupPath, "$PLUGINSDIR/app-64.7z"], {
      windowsHide: true,
    });
    const app7z = path.join(temp, "$PLUGINSDIR", "app-64.7z");

    if (result.status !== 0 || !fs.existsSync(app7z)) {
      fail(`не удалось извлечь app-64.7z из ${name}`);
      return;
    }

    const payload = path.join(temp, "payload");
    result = spawnSync(sevenZip, ["x", "-y", `-o${payload}`, app7z], {
      windowsHide: true,
      maxBuffer: 64 * 1024 * 1024,
    });

    const setupAsar = path.join(payload, "resources", "app.asar");
    if (result.status !== 0 || !fs.existsSync(setupAsar)) {
      fail(`не удалось извлечь app.asar из ${name}`);
      return;
    }

    checkAsarContent(setupAsar, `Setup ${name} app.asar`);

    if (unpackedInfo) {
      const setupSha = sha256(setupAsar);
      if (setupSha !== unpackedInfo.asarSha)
        fail(
          `app.asar внутри Setup (${setupSha.slice(0, 16)}…) не совпадает с ` +
            `win-unpacked (${unpackedInfo.asarSha.slice(0, 16)}…)`
        );
      else note(`Setup payload app.asar байт-в-байт совпадает с win-unpacked`);
    }

    const mainNode = path.join(
      payload,
      "resources",
      "app.asar.unpacked",
      "node_modules",
      "hlsr-native",
      "build",
      "Release",
      "main.node"
    );
    if (!fs.existsSync(mainNode)) fail(`в Setup нет app.asar.unpacked main.node`);

    const updateConfig = path.join(payload, "resources", "app-update.yml");
    if (!fs.existsSync(updateConfig)) {
      fail(`в Setup нет resources/app-update.yml — обновления не будут работать`);
    } else {
      const yml = fs.readFileSync(updateConfig, "utf8");
      if (!/provider:/.test(yml)) fail(`app-update.yml без provider`);
      else note(`app-update.yml на месте (${yml.trim().split(/\r?\n/).length} строк)`);
    }

    const setupVersion = asarPackageVersion(setupAsar);
    if (setupVersion && setupVersion !== pkg.version)
      fail(`внутри Setup упакована версия ${setupVersion} вместо ${pkg.version}`);

    checkInstallFiles(payload, pkg);
  } finally {
    try {
      fs.rmSync(temp, { recursive: true, force: true });
    } catch (e) {
      /* временный каталог */
    }
  }
}

// Установщик копирует ровно файлы из install-files.json и сверяет каждый:
// любое расхождение (например, переподписанный после составления списка
// HLSR.exe) у пользователя закончится отказом установки.
//
// resources/elevate.exe electron-builder дописывает в payload сам, уже
// после win-unpacked. В установку он не копируется: HLSR ставится без прав
// администратора, и повышать их нечему.
const PACKAGER_EXTRAS = new Set(["resources/elevate.exe"]);

function checkInstallFiles(payload, pkg) {
  const fileList = require("../src/main/windows-install/file-list");
  let list;
  try {
    list = fileList.read(payload);
  } catch (error) {
    fail(error.message);
    return;
  }
  if (list.version !== pkg.version) fail(`install-files.json описывает версию ${list.version} вместо ${pkg.version}`);
  if (list.app.appId !== pkg.build.appId) fail(`install-files.json: appId ${list.app.appId} вместо ${pkg.build.appId}`);

  const listed = new Set(list.files.map((item) => item.path));
  let bad = 0;
  for (const item of list.files) {
    const file = path.join(payload, ...item.path.split("/"));
    if (!fs.existsSync(file)) {
      if (bad++ < 5) fail(`в Setup нет файла из списка: ${item.path}`);
      continue;
    }
    if (fs.statSync(file).size !== item.size || sha256(file) !== item.sha256)
      if (bad++ < 5) fail(`файл Setup не совпадает со списком: ${item.path}`);
  }
  const extra = [];
  (function walk(relative) {
    for (const entry of fs.readdirSync(path.join(payload, relative), { withFileTypes: true })) {
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) walk(child);
      else if (child !== fileList.RELATIVE && !listed.has(child) && !PACKAGER_EXTRAS.has(child)) extra.push(child);
    }
  })("");
  if (extra.length) fail(`в Setup файлы вне install-files.json: ${extra.slice(0, 5).join(", ")}`);
  for (const required of ["resources/installer/index.html", "resources/installer/preload.js", "resources/app-update.yml"])
    if (!listed.has(required)) fail(`установщик без ${required}`);
  if (!bad && !extra.length)
    note(`install-files.json: ${list.files.length} файлов Setup совпадают по размеру и SHA-256`);
}

function main() {
  const args = process.argv.slice(2);
  const setupIndex = args.indexOf("--setup");
  const pkg = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8"));

  console.log(`Проверка свежести упакованного лаунчера (ожидается ${pkg.version})`);

  const unpackedInfo = checkWinUnpacked();

  if (setupIndex !== -1) {
    const setupPath = args[setupIndex + 1]
      ? path.resolve(args[setupIndex + 1])
      : path.join(ROOT, "build", `HLSR-Setup-${pkg.version}.exe`);
    checkSetup(setupPath, unpackedInfo);
  }

  for (const line of notes) console.log("  " + line);

  if (problems.length) {
    console.error("\nПроверка НЕ пройдена:");
    for (const problem of problems) console.error("  ✗ " + problem);
    process.exit(1);
  }

  console.log("\nПакет свежий: все маркеры нового кода на месте, версии совпадают.");
}

module.exports = { REQUIRED_MARKERS, FORBIDDEN_MARKERS, problems };

if (require.main === module) main();
