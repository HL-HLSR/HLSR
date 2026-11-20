"use strict";

// Оверлей LiveSplit внутри игры (resources/livesplit-overlay, исходники в
// scripts/livesplit-overlay). Лаунчер кладёт в папку игры прокси: d3d9.dll
// для Source (HL2, Ghosting) и opengl32.dll для GoldSrc (HL1). Игра грузит
// его сама, как обёртки вроде ReShade; прокси передаёт вызовы настоящей DLL
// и рисует снимок окна LiveSplit поверх каждого кадра, поэтому оверлей виден
// и в полноэкранном режиме. В другие процессы лаунчер ничего не внедряет.
//
// Прокси читает настройки (overlay.ini) и пишет журнал в папку данных игры,
// путь к которой лежит в hlsr-overlay.path рядом с ним. Сохранение
// персонализации переписывает overlay.ini, и запущенная игра подхватывает его
// на лету. Папка данных своя у каждой игры.
//
// Под Wine окно другого процесса не снимается, поэтому там прокси просит
// 64-битный помощник внедрить в LiveSplit hlsr-share64.dll, и кадр идёт через
// общую память. Эти два файла есть только в Linux-сборке лаунчера. Игре под
// Wine нужна ещё замена DLL (WINEDLLOVERRIDES=d3d9=n,b), см. environmentFor.
//
// LiveSplit, запущенный из лаунчера, открыт для одной игры (на Linux — в её
// префиксе Proton). В других играх оверлей тогда выключен в overlay.ini и
// включается снова, когда LiveSplit закроют или откроют для этой игры.

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { writeFileAtomic } = require("./atomic");
const overlaySettings = require("../../shared/livesplit-overlay");

const RESOURCE_DIRECTORY = "livesplit-overlay";
// Прокси: имя в ресурсах и имя, под которым игра загрузит его из своей папки.
const PROXIES = {
  d3d9: { resource: "proxy-d3d9.dll", name: "d3d9.dll" },
  opengl32: { resource: "proxy-opengl32.dll", name: "opengl32.dll" },
};
// Прокси нужны в обеих системах. Окно LiveSplit на Windows снимает сам лаунчер
// (livesplit-window-capture.js), под Wine — 64-битные помощник и DLL изнутри
// LiveSplit.
const HELPER64 = "hlsr-overlay64.exe";
const SHARE_DLL = "hlsr-share64.dll";
const PROXY_FILES = [PROXIES.d3d9.resource, PROXIES.opengl32.resource];
const FILES_WINDOWS = PROXY_FILES;
const FILES_LINUX = PROXY_FILES.concat([HELPER64, SHARE_DLL]);
// До 0.4.9 снимок делал этот exe, и его копия лежала в папке данных.
const LEGACY_CAPTURE = "hlsr-capture.exe";
const FILES = process.platform === "win32" ? FILES_WINDOWS : FILES_LINUX;
const SIDECAR = "hlsr-overlay.path";
// Строка внутри прокси (scripts/livesplit-overlay/proxy.h): по ней своя
// d3d9.dll отличается от DXVK или ReShade пользователя.
const PROXY_MARKER = Buffer.from("HLSR-OVERLAY-PROXY-1", "latin1");
const CONFIG = "overlay.ini";
const SNAPSHOT_MAGIC = "HLSRLS1\0";
// Окно LiveSplit больше 2048 пикселей DLL не снимает (MAX_SIDE в common.h).
const MAX_SIDE = 2048;

function sha256Of(fsModule, file) {
  return crypto.createHash("sha256").update(fsModule.readFileSync(file)).digest("hex");
}

// Имя процесса игры: hl2.exe, hl.exe.
function processNameOf(config) {
  return path.win32.basename(String(config.executable || "")).toLowerCase();
}

// Снимок от помощника (capture): "HLSRLS1\0", ширина, высота (uint32
// LE), затем строки BGRX сверху вниз.
function parseSnapshot(buffer) {
  if (!Buffer.isBuffer(buffer) || buffer.length < 16 || buffer.toString("latin1", 0, 8) !== SNAPSHOT_MAGIC) return null;
  const width = buffer.readUInt32LE(8), height = buffer.readUInt32LE(12);
  if (!width || !height || width > MAX_SIDE || height > MAX_SIDE || buffer.length !== 16 + width * height * 4) return null;
  return { width, height, pixels: buffer.subarray(16) };
}

// Код из файла статуса помощника под Proton (--status); null — не дождались.
// Сессия umu не завершается, пока в префиксе работает LiveSplit или игра,
// так что выход процесса ждать бесполезно.
async function waitForStatus(fsModule, file, timeoutMs, options = {}) {
  const deadline = Date.now() + timeoutMs;
  const interval = options.intervalMs || 200;
  for (;;) {
    if (options.cancelled && options.cancelled()) return null;
    try {
      const text = fsModule.readFileSync(file, "utf8").trim();
      if (/^\d+$/.test(text)) return Number(text);
    } catch (error) { /* файла ещё нет */ }
    if (Date.now() >= deadline) return null;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
}

// Значение WINEDLLOVERRIDES с добавленной заменой: уже заданные пользователем
// замены остаются.
function mergeOverrides(existing, addition) {
  const current = String(existing || "").trim();
  return current ? `${current};${addition}` : addition;
}

// В сборке файлы лежат в resources/livesplit-overlay (extraResources), из
// исходников (стенд, npm run dev) — в resources/ репозитория, как у render-host.js.
function defaultResourcesPath(files) {
  const candidates = [
    process.resourcesPath,
    path.resolve(__dirname, "..", "..", "resources"),
    path.resolve(__dirname, "..", "..", "..", "resources"),
  ].filter(Boolean);
  return candidates.find((dir) => files.existsSync(path.join(dir, RESOURCE_DIRECTORY))) || process.resourcesPath;
}

class LiveSplitOverlay {
  constructor(options = {}) {
    this.baseDir = options.baseDir;
    this.resourcesPath = options.resourcesPath || defaultResourcesPath(this.fs || fs);
    this.fs = options.fs || fs;
    this.logger = options.logger || { info() {}, warn() {} };
    this.platform = options.platform || process.platform;
    // Для какой игры запущен LiveSplit из лаунчера; null — не запущен.
    this.boundGameId = null;
  }

  // Файлы ресурсов для этой системы.
  files() {
    return this.platform === "win32" ? FILES_WINDOWS : FILES_LINUX;
  }

  // Путь так, как его видит игра: под Wine корень Linux — диск Z:.
  gamePath(file) {
    return this.platform === "win32" ? path.resolve(file) : `Z:${path.resolve(file).replace(/\//g, "\\")}`;
  }

  // overlay.ini игры: оверлей выключен, если LiveSplit открыт для другой игры.
  iniFor(gameId, overlay) {
    const bound = !this.boundGameId || this.boundGameId === gameId;
    return overlaySettings.toIni(Object.assign({}, overlay, { enabled: overlay.enabled && bound }));
  }

  dirFor(gameId) {
    if (!this.baseDir || !/^[a-z0-9-]+$/i.test(String(gameId))) return null;
    return path.join(this.baseDir, RESOURCE_DIRECTORY, gameId);
  }

  resource(name) {
    return path.join(this.resourcesPath, RESOURCE_DIRECTORY, name);
  }

  available() {
    return this.files().every((name) => this.fs.existsSync(this.resource(name)));
  }

  // GoldSrc рисует через OpenGL, Source — через Direct3D 9.
  proxyOf(config) {
    return config && config.engine === "goldsrc" ? PROXIES.opengl32 : PROXIES.d3d9;
  }

  // Куда игра смотрит за DLL. Source грузит shaderapidx9.dll из bin\ и ищет
  // его зависимости там, но не у всех сборок это так: прокси лежит и в bin\, и
  // рядом с hl2.exe. GoldSrc грузит hw.dll из папки hl.exe.
  proxyFolders(installPath, config) {
    const root = path.resolve(installPath);
    const executable = path.resolve(root, String(config.executable || ""));
    const folders = config && config.engine === "goldsrc"
      ? [path.dirname(executable)]
      : [path.join(root, "bin"), path.dirname(executable)];
    return folders.filter((folder, index) =>
      folders.indexOf(folder) === index && (folder === root || folder.startsWith(root + path.sep)));
  }

  // Копия файла, если её ещё нет или она устарела. Файл запущенной игры на
  // Windows заблокирован: тогда остаётся прежняя копия.
  copyTo(source, target) {
    const wanted = sha256Of(this.fs, source);
    let current = null;
    try { current = sha256Of(this.fs, target); } catch (error) { /* копии ещё нет */ }
    if (current === wanted) return target;
    const temporary = `${target}.${process.pid}.tmp`;
    try {
      this.fs.copyFileSync(source, temporary);
      this.fs.renameSync(temporary, target);
    } catch (error) {
      try { this.fs.rmSync(temporary, { force: true }); } catch (ignored) { /* нечего убирать */ }
      if (current === null) throw error;
      this.logger.warn(`[livesplit-overlay] ${path.basename(target)} не обновлён: ${error.message}`);
    }
    return target;
  }

  syncFile(name, dir) {
    return this.copyTo(this.resource(name), path.join(dir, name));
  }

  // Файл по этому пути — наш прокси (а не DXVK или ReShade пользователя).
  isOwnProxy(file) {
    try {
      return this.fs.readFileSync(file).includes(PROXY_MARKER);
    } catch (error) {
      return false;
    }
  }

  // Кладёт прокси и hlsr-overlay.path в папки игры. Возвращает папки, где
  // прокси встал. Чужую d3d9.dll или opengl32.dll не трогает.
  placeProxies(target, dataDir) {
    const kind = this.proxyOf(target.config);
    const source = this.resource(kind.resource);
    const placed = [];
    for (const folder of this.proxyFolders(target.installPath, target.config)) {
      if (!this.fs.existsSync(folder)) continue;
      const file = path.join(folder, kind.name);
      if (this.fs.existsSync(file) && !this.isOwnProxy(file)) {
        this.logger.warn(`[livesplit-overlay] ${file} не от лаунчера, оверлей в эту папку не ставится`);
        continue;
      }
      try {
        this.copyTo(source, file);
        const sidecar = path.join(folder, SIDECAR);
        const text = `${this.gamePath(dataDir)}\n`;
        let current = null;
        try { current = this.fs.readFileSync(sidecar, "utf8"); } catch (error) { /* файла ещё нет */ }
        if (current !== text) writeFileAtomic(sidecar, text);
        placed.push(folder);
      } catch (error) {
        this.logger.warn(`[livesplit-overlay] прокси ${file} не положен: ${error.message}`);
      }
    }
    return placed;
  }

  // Убирает свой прокси и hlsr-overlay.path из папок игры. Чужие файлы и
  // файл запущенной игры остаются.
  removeProxies(target) {
    const kind = this.proxyOf(target.config);
    for (const folder of this.proxyFolders(target.installPath, target.config)) {
      const file = path.join(folder, kind.name);
      const sidecar = path.join(folder, SIDECAR);
      try {
        if (this.fs.existsSync(file) && this.isOwnProxy(file)) this.fs.rmSync(file, { force: true });
        if (!this.fs.existsSync(file)) this.fs.rmSync(sidecar, { force: true });
      } catch (error) {
        this.logger.warn(`[livesplit-overlay] ${file} не убран: ${error.message}`);
      }
    }
  }

  // Готовит запуск игры: overlay.ini и помощники в папке данных, прокси в
  // папках игры. target — { installPath, config }. Зовётся до запуска игры:
  // прокси подхватывается при старте. null — оверлей выключен (прокси
  // убран). dllOverride — замена для WINEDLLOVERRIDES под Wine. proxy: true —
  // прокси нужен и без оверлея (Vulkan, vulkan-backend.js): он ставится, а
  // оверлей в overlay.ini выключен.
  prepare(gameId, settings, target = null, { proxy = false } = {}) {
    const overlay = overlaySettings.sanitizeOverlay(settings && settings.liveSplitOverlay);
    const dir = this.dirFor(gameId);
    if (!dir) return null;
    if (!overlay.enabled && !proxy) {
      if (target) this.removeProxies(target);
      return null;
    }
    if (!this.available()) throw new Error("файлы оверлея LiveSplit не найдены в ресурсах лаунчера");
    this.fs.mkdirSync(dir, { recursive: true });
    // Под Wine прокси запускает помощник из папки данных.
    if (this.platform !== "win32") {
      this.syncFile(HELPER64, dir);
      this.syncFile(SHARE_DLL, dir);
    }
    writeFileAtomic(path.join(dir, CONFIG), this.iniFor(gameId, overlay));
    const kind = this.proxyOf(target && target.config);
    const placed = target ? this.placeProxies(target, dir) : [];
    const dllOverride = this.platform !== "win32" && placed.length ? `${kind.name.replace(/\.dll$/i, "")}=n,b` : null;
    return { dir, log: path.join(dir, "overlay.log"), proxy: kind.name, placed, dllOverride };
  }

  // Настройки из окна персонализации. Если папка игры уже есть (игра
  // запускалась с оверлеем), прокси в запущенной игре подхватит их сразу.
  writeConfig(gameId, settings) {
    const dir = this.dirFor(gameId);
    if (!dir || !this.fs.existsSync(dir)) return false;
    const overlay = overlaySettings.sanitizeOverlay(settings && settings.liveSplitOverlay);
    writeFileAtomic(path.join(dir, CONFIG), this.iniFor(gameId, overlay));
    return true;
  }

  // LiveSplit запущен для gameId (или закрыт — null): overlay.ini всех игр,
  // которые уже запускались с оверлеем, переписывается. settingsOf(gameId) —
  // настройки игры из GameSettingsStore.
  bindTo(gameId, settingsOf) {
    const next = gameId || null;
    if (next === this.boundGameId) return false;
    this.boundGameId = next;
    const root = this.baseDir && path.join(this.baseDir, RESOURCE_DIRECTORY);
    let names = [];
    try { names = this.fs.readdirSync(root); } catch (error) { return true; }
    for (const name of names) {
      if (name === "capture" || !this.dirFor(name)) continue;
      try {
        this.writeConfig(name, settingsOf(name));
      } catch (error) {
        this.logger.warn(`[livesplit-overlay] overlay.ini ${name} не переписан: ${error.message}`);
      }
    }
    return true;
  }

  // Папка снимка окна LiveSplit (предпросмотр), отдельная, чтобы не зависеть от
  // игр. Linux: ещё помощник injector64 и shareDll внутри LiveSplit.
  captureTool() {
    const dir = this.dirFor("capture");
    if (!dir || !this.available()) return null;
    this.fs.mkdirSync(dir, { recursive: true });
    const tool = { dir, snapshot: path.join(dir, "snapshot.bin"), status: path.join(dir, "capture.status") };
    if (this.platform !== "win32") {
      tool.injector64 = this.syncFile(HELPER64, dir);
      tool.shareDll = this.syncFile(SHARE_DLL, dir);
    }
    return tool;
  }

  // Копия hlsr-capture.exe от прежней версии: антивирус продолжал бы находить
  // её в папке данных и после обновления.
  removeLegacyCapture() {
    const dir = this.dirFor("capture");
    if (!dir) return;
    try {
      this.fs.rmSync(path.join(dir, LEGACY_CAPTURE), { force: true });
    } catch (error) {
      this.logger.warn(`[livesplit-overlay] ${LEGACY_CAPTURE} не убран: ${error.message}`);
    }
  }

  waitForStatus(file, timeoutMs, options = {}) {
    return waitForStatus(this.fs, file, timeoutMs, options);
  }

  readSnapshot(file) {
    try {
      return parseSnapshot(this.fs.readFileSync(file));
    } catch (error) {
      return null;
    }
  }
}

module.exports = LiveSplitOverlay;
module.exports.processNameOf = processNameOf;
module.exports.parseSnapshot = parseSnapshot;
module.exports.waitForStatus = waitForStatus;
module.exports.mergeOverrides = mergeOverrides;
module.exports.RESOURCE_DIRECTORY = RESOURCE_DIRECTORY;
module.exports.PROXIES = PROXIES;
module.exports.HELPER64 = HELPER64;
module.exports.SHARE_DLL = SHARE_DLL;
module.exports.SIDECAR = SIDECAR;
module.exports.PROXY_MARKER = PROXY_MARKER;
module.exports.FILES = FILES;
module.exports.FILES_WINDOWS = FILES_WINDOWS;
module.exports.FILES_LINUX = FILES_LINUX;
