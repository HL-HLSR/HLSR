"use strict";

// Апдейтер Windows в составе app.asar: dist/electron/updater
// (src/main/launcher-update/updater-mode.js объясняет, почему не
// resources/updater). Раскладка та же, что была в resources/updater:
// src/updater без shared/, а на месте shared/<путь> — сам модуль
// src/main/<путь> (в репозитории shared/* лишь ссылаются на него).
// JS сжимается на месте, как раньше в build-windows-setup.js.
const fs = require("fs");
const path = require("path");
const { minifyDir, summary } = require("../minify-js");

const ROOT = path.resolve(__dirname, "../..");
const SOURCE = path.join(ROOT, "src", "updater");
const MAIN = path.join(ROOT, "src", "main");
const BROWSER_SCRIPTS = ["window/updater.js"];

function filesUnder(dir, base = dir) {
  const found = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) found.push(...filesUnder(full, base));
    else if (entry.isFile()) found.push(path.relative(base, full).split(path.sep).join("/"));
  }
  return found.sort();
}

// shared/<путь> → src/main/<путь>.
function sharedModules(source = SOURCE) {
  return filesUnder(path.join(source, "shared")).map((relative) => ({
    to: "shared/" + relative,
    from: path.join(MAIN, relative.split("/").join(path.sep)),
  }));
}

// Раскладывает апдейтер в outDir (каталог пересоздаётся). Без сжатия.
function stage(outDir, { source = SOURCE, channel = process.env.HLSR_BUILD_CHANNEL } = {}) {
  fs.rmSync(outDir, { recursive: true, force: true });
  fs.cpSync(source, outDir, {
    recursive: true,
    filter: (file) => path.relative(source, file).split(path.sep)[0] !== "shared",
  });
  for (const item of sharedModules(source)) {
    if (!fs.existsSync(item.from)) throw new Error(`апдейтер: нет модуля ${path.relative(ROOT, item.from)} для ${item.to}`);
    const target = path.join(outDir, item.to.split("/").join(path.sep));
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(item.from, target);
  }
  // Как HLSR_BUILD_CHANNEL в webpack: канал вшивается при сборке, а не читается у пользователя.
  fs.writeFileSync(path.join(outDir, "build-channel.json"), JSON.stringify({ channel: channel === "qa" ? "qa" : "production" }));
  return outDir;
}

// Сборка для app.asar: раскладка и минификация.
async function build(outDir) {
  stage(outDir);
  const results = await minifyDir(outDir, { browserScripts: BROWSER_SCRIPTS });
  return summary("апдейтер", results);
}

module.exports = { stage, build, sharedModules, BROWSER_SCRIPTS, SOURCE };
