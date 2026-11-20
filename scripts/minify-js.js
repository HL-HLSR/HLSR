"use strict";

// Минификация JS, который уходит в сборки обычными файлами, мимо webpack:
// апдейтер Windows (dist/electron/updater в app.asar), окно установщика (resources/installer)
// и модули Linux AppImage-установщика. Каждый .js сжимается на месте, поэтому
// раскладка и относительные require() не меняются. Как и в main-бандле
// лаунчера, комментарии убираются, локальные имена сокращаются, а имена
// функций и классов остаются для логов и отчётов о сбоях. HTML и CSS не
// трогаем.

const fs = require("fs");
const path = require("path");
const { minify } = require("terser");

function jsFiles(dir, base = dir) {
  const found = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) found.push(...jsFiles(full, base));
    else if (entry.isFile() && entry.name.endsWith(".js")) found.push(path.relative(base, full).split(path.sep).join("/"));
  }
  return found.sort();
}

// browser: обычный <script> окна — его верхнеуровневые имена глобальные,
// их не сокращаем. Остальные файлы — модули CommonJS.
async function minifyFile(file, relative, browser) {
  const source = fs.readFileSync(file, "utf8");
  const result = await minify(source, {
    module: false,
    toplevel: !browser,
    compress: { passes: 2 },
    keep_classnames: true,
    keep_fnames: true,
    format: { comments: false },
  });
  if (typeof result.code !== "string" || !result.code) throw new Error(`Terser вернул пустой результат: ${relative}`);
  fs.writeFileSync(file, result.code);
  return { relative, before: Buffer.byteLength(source), after: Buffer.byteLength(result.code) };
}

// Сжимает все .js в dir. browserScripts — пути относительно dir.
async function minifyDir(dir, { browserScripts = [] } = {}) {
  if (!fs.statSync(dir, { throwIfNoEntry: false })?.isDirectory()) throw new Error(`Нет папки для минификации: ${dir}`);
  const browser = new Set(browserScripts);
  const results = [];
  for (const relative of jsFiles(dir)) {
    try {
      results.push(await minifyFile(path.join(dir, relative), relative, browser.has(relative)));
    } catch (error) {
      throw new Error(`Не удалось минифицировать ${relative} в ${dir}: ${error.message}`);
    }
  }
  return results;
}

function summary(label, results) {
  const sum = (key) => results.reduce((total, item) => total + item[key], 0);
  return `Минификация (${label}): ${results.length} файлов, ${sum("before")} -> ${sum("after")} байт`;
}

module.exports = { minifyDir, jsFiles, summary };
