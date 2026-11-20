"use strict";

// Загрузка замороженных модулей обновлялки (набор 0.4.7 или 0.4.9, frozen.json) так, будто они
// работают на другой ОС: process.platform подменён, require("path") — path.win32 или path.posix.
// Нужно, чтобы одну и ту же проверку обновлялки (plan.js — пути Windows, LauncherUpdateGate —
// каналы Linux) можно было прогнать на любой машине. Модули внутри каталога набора загружаются
// тоже в копии для этой ОС (отдельный кеш на платформу); встроенные модули Node и всё остальное —
// обычным require.
//
// Для своей ОС модуль грузится обычным require.

const fs = require("fs");
const path = require("path");
const vm = require("vm");
const Module = require("module");
const { setDir } = require("../freeze");

const caches = new Map();

function loadFile(file, platform, root) {
  if (!caches.has(platform)) caches.set(platform, new Map());
  const cache = caches.get(platform);
  if (cache.has(file)) return cache.get(file).exports;

  const module = { exports: {} };
  cache.set(file, module);
  const fakeProcess = new Proxy(process, {
    get(target, key) {
      if (key === "platform") return platform;
      const value = Reflect.get(target, key);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
  const nativeRequire = Module.createRequire(file);
  const localRequire = (request) => {
    if (request === "path") return platform === "win32" ? path.win32 : path.posix;
    if (!request.startsWith(".")) return nativeRequire(request);
    const resolved = nativeRequire.resolve(request);
    if (!resolved.startsWith(root + path.sep)) return nativeRequire(request);
    return loadFile(resolved, platform, root);
  };
  const compiled = vm.compileFunction(
    fs.readFileSync(file, "utf8"),
    ["exports", "require", "module", "__filename", "__dirname", "process"],
    { filename: file }
  );
  try {
    compiled.call(module.exports, module.exports, localRequire, module, file, path.dirname(file), fakeProcess);
  } catch (error) {
    // Как require: модуль, бросивший при загрузке, не остаётся в кеше недоинициализированным.
    cache.delete(file);
    throw error;
  }
  return module.exports;
}

/**
 * Замороженный модуль набора set ("0.4.7", "0.4.9") по пути от src/
 * ("main/launcher-update/launcher-channel.js"), загруженный для ОС platform ("win32" или "linux").
 * Набор обязателен: без него setDir бросает, а не подставляет 0.4.7 молча.
 */
function loadAs(relative, platform, set) {
  const root = setDir(set);
  const file = path.join(root, ...relative.split("/"));
  if (!fs.existsSync(file)) throw new Error(`нет замороженного модуля: ${relative}`);
  return platform === process.platform ? require(file) : loadFile(file, platform, root);
}

module.exports = { loadAs };
