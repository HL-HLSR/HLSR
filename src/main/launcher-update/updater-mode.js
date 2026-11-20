"use strict";

const path = require("path");

// Режим внешнего апдейтера Windows. Копия HLSR.exe (HLSRUpdater.exe, см.
// updater-launch.js) запускается со своим app.asar — копией app.asar
// лаунчера — и флагом FLAG. Отдельного app апдейтеру нельзя: fuse
// onlyLoadAppFromAsar не даёт загрузить resources/app, а проверка
// целостности asar (enableEmbeddedAsarIntegrityValidation) роняет процесс
// на любом asar, чей заголовок не совпал с зашитым в exe. Поэтому апдейтер
// лежит внутри app.asar рядом с main-бандлом (dist/electron/updater,
// scripts/lib/updater-bundle.js), а точка входа (entry.js) по флагу
// загружает его вместо лаунчера.
const FLAG = "--hlsr-updater";
const DIR = "updater";
// Прежний апдейтер был отдельным app с productName «HLSR Updater»: тот же
// каталог данных Chromium, что и у него, и не тот, что у лаунчера.
const APP_NAME = "HLSR Updater";

function isUpdaterInvocation(argv, platform = process.platform) {
  return platform === "win32" && Array.isArray(argv) && argv.slice(1).includes(FLAG);
}

// bundleDir — каталог main-бандла (dist/electron внутри app.asar).
function mainPath(bundleDir = __dirname) {
  return path.join(bundleDir, DIR, "main.js");
}

function run(options = {}) {
  const electron = options.electron || require("electron");
  const app = electron.app;
  app.setName(APP_NAME);
  app.setPath("userData", path.join(app.getPath("appData"), APP_NAME));
  // eval("require"): webpack не должен вшивать апдейтер в main-бандл, он
  // лежит рядом обычными файлами.
  const load = options.require || eval("require");
  return load(mainPath(options.bundleDir));
}

module.exports = { FLAG, DIR, APP_NAME, isUpdaterInvocation, mainPath, run };
