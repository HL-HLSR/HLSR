"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const runsScan = require("./runs-scan");

const ARCHIVE_NAME = /^(hl1|ghosting)-demos-[0-9-]+\.zip$/;

function isArchiveName(name) {
  return typeof name === "string" && ARCHIVE_NAME.test(name);
}

// Где игра хранит демо: HL1 — её runs (папка мода и языковые папки), Ghosting — demoDirs.
function demoSources(config, installPath) {
  if (config && Array.isArray(config.demoDirs))
    return { dirs: config.demoDirs.slice(), extension: config.demoExtension || ".dem" };
  if (config && config.runsPath && config.runsExtension === ".dem") {
    const dirs = config.runsLanguageDirs ? runsScan.runDirs(installPath, config.runsPath) : [config.runsPath];
    return { dirs, extension: config.runsExtension };
  }
  return { dirs: [], extension: ".dem" };
}

// Все демо игры, новые сверху. Одинаковые имена в разных папках остаются оба.
function listDemos(config, installPath) {
  const { dirs, extension } = demoSources(config, installPath);
  const demos = [];
  for (const dir of dirs) {
    const result = runsScan.scanMany(installPath, [dir], extension);
    if (result.status !== "ok") continue;
    for (const item of result.items)
      demos.push({ dir, name: item.name, size: item.size, mtimeMs: Date.parse(item.modifiedAt) });
  }
  return demos.sort((a, b) => b.mtimeMs - a.mtimeMs || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

const two = (n) => String(n).padStart(2, "0");

function archiveName(slug, date, taken) {
  const stamp = `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())}-${two(date.getHours())}${two(date.getMinutes())}`;
  const base = `${slug}-demos-${stamp}`;
  let name = `${base}.zip`;
  for (let n = 2; taken.has(name); n++) name = `${base}-${n}.zip`;
  return name;
}

// Только то, что есть в свежем сканировании: точное совпадение по dir и name.
function pickItems(requested, scanned) {
  if (!Array.isArray(requested) || !requested.length) return { ok: false, code: "NO_ITEMS" };
  const items = [];
  for (const wanted of requested) {
    const found = wanted && scanned.find((item) => item.dir === wanted.dir && item.name === wanted.name);
    if (!found) return { ok: false, code: "DEMO_NOT_FOUND" };
    if (!items.includes(found)) items.push(found);
  }
  return { ok: true, items };
}

// Имя в архиве — имя файла; повтор получает префикс папки, а если и оно занято — -2, -3 перед расширением.
function entryNames(items) {
  const used = new Set(items.map((item) => item.name.toLowerCase()));
  const seen = new Set();
  return items.map((item) => {
    let entry = item.name;
    if (seen.has(entry.toLowerCase())) {
      const base = `${path.posix.basename(item.dir)}_${item.name}`;
      const ext = path.extname(base);
      entry = base;
      for (let n = 2; used.has(entry.toLowerCase()) || seen.has(entry.toLowerCase()); n++)
        entry = `${base.slice(0, base.length - ext.length)}-${n}${ext}`;
    }
    seen.add(entry.toLowerCase());
    return { item, entry };
  });
}

async function packDemos({ installPath, items, archivesDir, slug, now, archive, onProgress }) {
  fs.mkdirSync(archivesDir, { recursive: true });
  // Остатки прерванной упаковки; занятая папка не должна ломать новую упаковку.
  for (const old of fs.readdirSync(archivesDir)) {
    if (!old.startsWith(".staging-")) continue;
    try { fs.rmSync(path.join(archivesDir, old), { recursive: true, force: true }); } catch (_) { /* уберётся в следующий раз */ }
  }
  const name = archiveName(slug, now, new Set(fs.readdirSync(archivesDir)));
  const zipPath = path.join(archivesDir, name);
  const staging = path.join(archivesDir, `.staging-${crypto.randomBytes(6).toString("hex")}`);
  try {
    fs.mkdirSync(staging);
    const entries = entryNames(items);
    for (const { item, entry } of entries) {
      const source = path.join(installPath, ...item.dir.split("/"), item.name);
      const target = path.join(staging, entry);
      try {
        fs.linkSync(source, target);
      } catch (error) {
        try {
          fs.copyFileSync(source, target, fs.constants.COPYFILE_EXCL);
        } catch (copyError) {
          throw copyError.code === "ENOENT" ? Object.assign(copyError, { code: "DEMO_NOT_FOUND" }) : copyError;
        }
      }
    }
    // Список файлов для 7za лежит в staging: после сбоя его уберёт чистка .staging-*, а не останется в Demos.
    await archive.create(zipPath, { cwd: staging, listDir: staging, files: entries.map((e) => e.entry), type: "zip", onProgress }).promise;
    return { name, size: fs.statSync(zipPath).size };
  } catch (error) {
    fs.rmSync(zipPath, { force: true });
    throw error;
  } finally {
    fs.rmSync(staging, { recursive: true, force: true });
  }
}

module.exports = { ARCHIVE_NAME, isArchiveName, demoSources, listDemos, archiveName, pickItems, entryNames, packDemos };
