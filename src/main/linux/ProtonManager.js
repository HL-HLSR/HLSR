"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const crypto = require("crypto");
const { spawn } = require("child_process");
const { EventEmitter } = require("events");
const { pipeline } = require("stream/promises");
const { Transform } = require("stream");
const vdf = require("../game-delivery/vdf");
const netFallback = require("../net-fallback");

const RELEASE_TAG = "wineland-11.0-20261003-hlsr3";
const RELEASE_API = `https://api.github.com/repos/kanoyo-git/proton-cachyos/releases/tags/${RELEASE_TAG}`;
// Новые сборки Wineland HLSR выходят релизами того же репозитория. Список
// читается один раз за запуск лаунчера: у API GitHub без токена 60 запросов
// в час на адрес.
const RELEASES_API = "https://api.github.com/repos/kanoyo-git/proton-cachyos/releases?per_page=20";
const WINELAND_TAG = /^wineland-(\d+\.\d+)-(\d{8})-hlsr(\d+)$/;
const SHA512SUM_LINE = /^([0-9a-f]{128}) [ *]?([^\s/]+)\s*$/;
const MAX_ARCHIVE_BYTES = 1024 * 1024 * 1024;
const PINNED_ARCHIVE = Object.freeze({
  name: "proton-wineland-11.0-20261003-hlsr3-x86_64.tar.xz",
  size: 415958952,
  sha256: "f93af495521e5201608f2b4cf83d2efe853b29897e34a7462e1272e3ccce7fd5",
  sha512: "abd5828f534f2d786c68a80f5cb1b204d4825e50c6b636edb81f8010fa2da23c09b425b0dbd312b09064df12950124257d9c308fb93d128a11c17ce67771a6db",
});
const WINELAND_ARCHIVE = /^proton-wineland-\d+\.\d+-\d{8}-hlsr\d+-x86_64\.tar\.xz$/;
const WINELAND_DIRECTORY = /^proton-wineland-\d+\.\d+-\d{8}-hlsr\d+-x86_64$/;
const LEGACY_CACHYOS_DIRECTORY = /^proton-cachyos-/;
const UPSTREAM_WINELAND_DIRECTORY = /^proton-wineland-\d+\.\d+-\d{8}-x86_64$/;
const ALLOWED_HOSTS = new Set([
  "api.github.com", "github.com", "release-assets.githubusercontent.com",
  "objects.githubusercontent.com",
]);

function request(url, redirects = 0, signal) {
  return new Promise((resolve, reject) => {
    let parsed;
    try { parsed = new URL(url); } catch (error) { reject(error); return; }
    if (parsed.protocol !== "https:" || !ALLOWED_HOSTS.has(parsed.hostname) || redirects > 5) {
      reject(new Error("Недоверенный адрес загрузки Proton"));
      return;
    }
    const req = https.get(parsed, {
      headers: { "User-Agent": "HLSR-Linux", Accept: "application/vnd.github+json" },
      timeout: 30000,
      signal,
    }, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        if (!response.headers.location) {
          reject(new Error("GitHub вернул перенаправление без адреса"));
          return;
        }
        request(new URL(response.headers.location, parsed).href, redirects + 1, signal).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`GitHub ответил ${response.statusCode}`));
        return;
      }
      resolve(response);
    });
    req.on("timeout", () => req.destroy(new Error("Тайм-аут загрузки Proton")));
    req.on("error", reject);
  });
}

async function readText(url, limit = 2 * 1024 * 1024, signal) {
  let response;
  try {
    response = await request(url, 0, signal);
  } catch (error) {
    // Метаданные релиза при TLS-ошибке Node перечитываем через Chromium
    // (src/main/net-fallback.js). Сам архив сверяется по хешам, поэтому
    // для него это не нужно: его качает тот же стек, что и раньше.
    if (!netFallback.isTlsError(error) || !netFallback.electronNet() || !ALLOWED_HOSTS.has(new URL(url).hostname))
      throw error;
    const fallback = await netFallback.chromiumGet(url, {
      headers: { "User-Agent": "HLSR-Linux", Accept: "application/vnd.github+json" }, signal, limit });
    if (!ALLOWED_HOSTS.has(new URL(fallback.url).hostname))
      throw new Error("Недоверенный адрес загрузки Proton", { cause: error });
    if (fallback.truncated) throw new Error("Ответ GitHub слишком велик", { cause: error });
    if (fallback.status !== 200) throw new Error(`GitHub ответил ${fallback.status}`, { cause: error });
    return fallback.body.toString("utf8");
  }
  const chunks = [];
  let size = 0;
  for await (const chunk of response) {
    size += chunk.length;
    if (size > limit) throw new Error("Ответ GitHub слишком велик");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function run(file, args, signal) {
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { stdio: ["ignore", "pipe", "pipe"], signal });
    let errorText = "";
    child.stderr.on("data", (chunk) => { errorText += chunk.toString().slice(0, 2048); });
    child.on("error", (error) => { if (error.name !== "AbortError") reject(error); });
    child.on("close", (code) => {
      if (signal && signal.aborted) return reject(signal.reason);
      code === 0 ? resolve() : reject(new Error(`${file}: ${errorText.trim() || code}`));
    });
  });
}

function commandOutput(file, args, signal) {
  return new Promise((resolve, reject) => {
    const child = spawn("tar", [...args, file], { stdio: ["ignore", "pipe", "pipe"], signal });
    let output = "";
    let failed = false;
    child.stdout.on("data", (chunk) => {
      output += chunk.toString();
      if (output.length > 16 * 1024 * 1024) {
        failed = true;
        child.kill();
      }
    });
    child.on("error", (error) => { if (error.name !== "AbortError") reject(error); });
    child.on("close", (code) => {
      if (signal && signal.aborted) return reject(signal.reason);
      if (failed || code !== 0) return reject(new Error("Список файлов Proton-архива не читается"));
      resolve(output);
    });
  });
}

function validateArchiveListing(namesText, verboseText, expectedRoot, rootPattern = WINELAND_DIRECTORY) {
  if (!rootPattern.test(expectedRoot))
    throw new Error("Некорректный корневой каталог Proton-архива");

  const names = namesText.split(/\r?\n/).filter(Boolean);
  const verboseLines = verboseText.split(/\r?\n/).filter(Boolean);
  const rootPrefix = `${expectedRoot}/`;
  if (!names.length || names.some((name) =>
    // eslint-disable-next-line no-control-regex
    path.posix.isAbsolute(name) || name.includes("\\") || /[\x00-\x1f\x7f]/.test(name) ||
    name.includes(" -> ") || name.split("/").includes("..") ||
    (name !== expectedRoot && !name.startsWith(rootPrefix))))
    throw new Error("Proton-архив содержит недопустимые пути");
  if (verboseLines.length !== names.length)
    throw new Error("Списки файлов Proton-архива не совпадают");
  const nameSet = new Set(names);

  for (const line of verboseLines) {
    const type = line[0];
    if (!["-", "d", "l"].includes(type))
      throw new Error("Proton-архив содержит специальные файлы или жёсткие ссылки");
    if (type !== "l") continue;

    const start = line.indexOf(rootPrefix);
    const separator = line.lastIndexOf(" -> ");
    if (start < 0 || separator < start)
      throw new Error("Символическая ссылка Proton-архива не читается");

    const name = line.slice(start, separator);
    const target = line.slice(separator + 4);
    if (!nameSet.has(name))
      throw new Error("Символическая ссылка отсутствует в списке Proton-архива");
    const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(name), target));
    if (!target || path.posix.isAbsolute(target) || target.includes("\\") ||
        // eslint-disable-next-line no-control-regex
        /[\x00-\x1f\x7f]/.test(target) ||
        (resolved !== expectedRoot && !resolved.startsWith(rootPrefix)))
      throw new Error("Символическая ссылка Proton-архива выходит за каталог установки");
  }
  return names;
}

async function listArchive(file, expectedRoot, signal) {
  const results = await Promise.allSettled([
    commandOutput(file, ["-tJf"], signal),
    commandOutput(file, ["-tvJf"], signal),
  ]);
  for (const result of results) if (result.status === "rejected") throw result.reason;
  const [names, verbose] = results.map((result) => result.value);
  return validateArchiveListing(names, verbose, expectedRoot);
}

function safeReleaseAsset(release) {
  const assets = Array.isArray(release && release.assets) ? release.assets : [];
  if (!release || release.tag_name !== RELEASE_TAG)
    throw new Error("GitHub вернул неожиданный релиз Proton Wineland");
  const archive = assets.find((asset) => asset.name === PINNED_ARCHIVE.name);
  if (!archive || !archive.browser_download_url || archive.size !== PINNED_ARCHIVE.size)
    throw new Error("Закреплённый архив Proton Wineland отсутствует или заменён");
  if (archive.digest && archive.digest.toLowerCase() !== `sha256:${PINNED_ARCHIVE.sha256}`)
    throw new Error("GitHub сообщил другой SHA-256 закреплённого Proton-архива");
  return { archive };
}

// Версия сборки из тега или каталога: дата сборки и номер HLSR.
function winelandVersion(name) {
  const match = WINELAND_TAG.exec(name) ||
    /^proton-wineland-(\d+\.\d+)-(\d{8})-hlsr(\d+)-x86_64$/.exec(name);
  if (!match) return null;
  return { wine: match[1], date: match[2], build: Number(match[3]),
    tag: `wineland-${match[1]}-${match[2]}-hlsr${match[3]}` };
}

function compareWineland(a, b) {
  if (a.date !== b.date) return a.date < b.date ? -1 : 1;
  if (a.build !== b.build) return a.build - b.build;
  return a.wine.localeCompare(b.wine, undefined, { numeric: true });
}

// Новый релиз проверяется строже, чем «есть архив»: имя архива обязано
// совпадать с тегом, GitHub обязан сообщить его SHA-256, а рядом обязан
// лежать .sha512sum. Без любого из трёх релиз пропускается.
function updateCandidate(release) {
  if (!release || release.draft || release.prerelease) return null;
  const version = winelandVersion(release.tag_name);
  if (!version || version.tag !== release.tag_name) return null;
  const assets = Array.isArray(release.assets) ? release.assets : [];
  const name = `proton-wineland-${version.wine}-${version.date}-hlsr${version.build}-x86_64.tar.xz`;
  const archive = assets.find((asset) => asset && asset.name === name);
  const sums = assets.find((asset) => asset && asset.name === name.replace(/\.tar\.xz$/, ".sha512sum"));
  if (!archive || !sums || !archive.browser_download_url || !sums.browser_download_url) return null;
  if (!Number.isSafeInteger(archive.size) || archive.size <= 0 || archive.size > MAX_ARCHIVE_BYTES)
    return null;
  const digest = /^sha256:([0-9a-f]{64})$/.exec(String(archive.digest || "").toLowerCase());
  if (!digest) return null;
  return { version, archive, sums, sha256: digest[1] };
}

function parseSha512sum(text, name) {
  for (const line of String(text).split(/\r?\n/)) {
    const match = SHA512SUM_LINE.exec(line.trim());
    if (match && match[2] === name) return match[1];
  }
  throw new Error("В .sha512sum нет строки для архива Proton Wineland");
}

// Каталог Proton занят, если из него запущен хоть один процесс: игра,
// LiveSplit или сам wineserver. Такой каталог удалять нельзя.
function directoryInUse(dir) {
  const prefix = path.resolve(dir) + path.sep;
  let entries;
  try { entries = fs.readdirSync("/proc"); } catch (error) { return false; }
  for (const pid of entries) {
    if (!/^\d+$/.test(pid)) continue;
    try {
      if (fs.readlinkSync(`/proc/${pid}/exe`).startsWith(prefix)) return true;
    } catch (error) { /* чужой или завершившийся процесс */ }
    try {
      if (fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0").some((arg) => arg.startsWith(prefix)))
        return true;
    } catch (error) { /* чужой или завершившийся процесс */ }
  }
  return false;
}

function isProton(root) {
  try {
    const executable = path.join(root, "proton");
    fs.accessSync(executable, fs.constants.X_OK);
    return fs.statSync(executable).isFile();
  } catch (error) { return false; }
}

class ProtonManager extends EventEmitter {
  constructor(options = {}) {
    super();
    this.dataDir = options.dataDir;
    this.home = options.home || os.homedir();
    this.logger = options.logger || { info() {}, warn() {}, error() {} };
    this.root = path.join(this.dataDir, "protons");
    this.selectionFile = path.join(this.dataDir, "proton-selection.json");
    this.state = { phase: "idle", percent: 0, error: null, operation: null };
    this.pending = null;
    this.controller = null;
    this.readText = options.readText || readText;
    this.request = options.request || request;
    this.inUse = options.inUse || directoryInUse;
    this.update = { checked: false, available: false, latest: null, updatedFrom: null, updatedTo: null, error: null };
    this.candidate = null;
    this.updateCheck = null;
  }

  setState(next) {
    this.state = Object.assign({}, this.state, next);
    this.emit("state", this.status());
  }

  searchRoots() {
    const home = this.home;
    const roots = [
      this.root,
      path.join(home, ".steam/root/compatibilitytools.d"),
      path.join(home, ".steam/steam/compatibilitytools.d"),
      path.join(home, ".local/share/Steam/compatibilitytools.d"),
      path.join(home, ".var/app/com.valvesoftware.Steam/data/Steam/compatibilitytools.d"),
      path.join(home, ".local/share/Steam/steamapps/common"),
      path.join(home, ".steam/root/steamapps/common"),
      "/usr/share/steam/compatibilitytools.d",
      "/usr/local/share/steam/compatibilitytools.d",
      path.join(home, ".config/heroic/tools/proton"),
      path.join(home, ".var/app/com.heroicgameslauncher.hgl/config/heroic/tools/proton"),
    ];
    for (const steam of [
      path.join(home, ".local/share/Steam"),
      path.join(home, ".steam/root"),
      path.join(home, ".var/app/com.valvesoftware.Steam/data/Steam"),
    ]) {
      try {
        const file = path.join(steam, "steamapps", "libraryfolders.vdf");
        const folders = vdf.getInsensitive(vdf.parse(fs.readFileSync(file, "utf8")), "libraryfolders");
        for (const key of Object.keys(folders || {})) {
          if (!/^\d+$/.test(key)) continue;
          const entry = folders[key];
          const value = typeof entry === "string" ? entry : vdf.getInsensitive(entry, "path");
          if (typeof value === "string" && path.isAbsolute(value))
            roots.push(path.join(value, "steamapps", "common"));
        }
      } catch (error) { /* library list may be absent */ }
    }
    return roots;
  }

  discover() {
    const found = new Map();
    const add = (candidate, source) => {
      let real;
      try { real = fs.realpathSync(candidate); } catch (error) { return; }
      if (!isProton(real) || found.has(real)) return;
      const managed = real.startsWith(path.resolve(this.root) + path.sep);
      const wineland = managed && WINELAND_DIRECTORY.test(path.basename(real));
      found.set(real, {
        id: real,
        name: path.basename(candidate),
        path: real,
        source: wineland ? "HLSR / Proton Wineland" : source,
        recommended: wineland,
      });
    };
    for (const root of this.searchRoots()) {
      let entries;
      try { entries = fs.readdirSync(root, { withFileTypes: true }); } catch (error) { continue; }
      for (const entry of entries) {
        if (!entry.isDirectory() && !entry.isSymbolicLink()) continue;
        add(path.join(root, entry.name), root === this.root ? "HLSR" : "Steam / system");
      }
    }
    for (const candidate of this.selectionData().customPaths || []) add(candidate, "Custom");
    // Сборки HLSR идут первыми и от новой к старой: при запасном выборе
    // лаунчер берёт первую из них.
    return Array.from(found.values()).sort((a, b) =>
      Number(b.recommended) - Number(a.recommended) ||
      (a.recommended ? b.name.localeCompare(a.name) : a.name.localeCompare(b.name)));
  }

  selectedPath() {
    return this.selectionData().path;
  }

  selectionData() {
    try {
      const value = JSON.parse(fs.readFileSync(this.selectionFile, "utf8"));
      return {
        path: typeof value.path === "string" ? value.path : null,
        customPaths: Array.isArray(value.customPaths)
          ? value.customPaths.filter((item) => typeof item === "string" && path.isAbsolute(item))
          : [],
      };
    } catch (error) { return { path: null, customPaths: [] }; }
  }

  writeSelection(data) {
    fs.mkdirSync(this.dataDir, { recursive: true });
    const temp = `${this.selectionFile}.${process.pid}.tmp`;
    fs.writeFileSync(temp, JSON.stringify(data, null, 2), { mode: 0o600 });
    fs.renameSync(temp, this.selectionFile);
  }

  selected() {
    const installed = this.discover();
    const pathChoice = this.selectedPath();
    const selected = installed.find((item) => item.path === pathChoice);
    if (selected) {
      const name = path.basename(selected.path);
      const managed = selected.path.startsWith(path.resolve(this.root) + path.sep);
      if (!managed || (!LEGACY_CACHYOS_DIRECTORY.test(name) &&
          !UPSTREAM_WINELAND_DIRECTORY.test(name))) return selected;
      return installed.find((item) => item.recommended) || null;
    }
    return installed.find((item) => item.recommended) || installed[0] || null;
  }

  select(rawPath) {
    const item = this.discover().find((entry) => entry.path === rawPath);
    if (!item) throw new Error("Выбранный Proton не найден. Обновите список версий.");
    this.writeSelection(Object.assign(this.selectionData(), { path: item.path }));
    this.emit("state", this.status());
    return item;
  }

  addCustom(rawPath) {
    if (typeof rawPath !== "string" || !path.isAbsolute(rawPath))
      throw new Error("Укажите абсолютный путь к папке Proton");
    let real;
    try { real = fs.realpathSync(rawPath); } catch (error) { throw new Error("Папка Proton не найдена", { cause: error }); }
    if (!isProton(real)) throw new Error("В папке нет исполняемого файла proton");
    const data = this.selectionData();
    if (!data.customPaths.includes(real)) data.customPaths.push(real);
    this.writeSelection(data);
    const item = this.discover().find((entry) => entry.path === real);
    this.emit("state", this.status());
    return item;
  }

  status() {
    const installed = this.discover();
    const selected = this.selected();
    return { installed, selected: selected ? selected.path : null, ...this.state,
      update: Object.assign({}, this.update) };
  }

  managedWineland() {
    let entries;
    try { entries = fs.readdirSync(this.root, { withFileTypes: true }); } catch (error) { return []; }
    return entries
      .filter((entry) => entry.isDirectory() && WINELAND_DIRECTORY.test(entry.name))
      .map((entry) => ({ name: entry.name, path: path.join(this.root, entry.name),
        version: winelandVersion(entry.name) }))
      .filter((entry) => entry.version && isProton(entry.path))
      .sort((a, b) => compareWineland(b.version, a.version));
  }

  async latestRelease(signal) {
    const list = JSON.parse(await this.readText(RELEASES_API, undefined, signal));
    if (!Array.isArray(list)) throw new Error("GitHub вернул неожиданный список релизов Proton");
    return list.map(updateCandidate).filter(Boolean)
      .sort((a, b) => compareWineland(b.version, a.version))[0] || null;
  }

  // Одна проверка метаданных за запуск. Загрузка — только через applyUpdate.
  checkForUpdate() {
    if (!this.updateCheck) this.updateCheck = this.runUpdateCheck().catch((error) => {
      this.update = Object.assign({}, this.update, { checked: true, error: error.message });
      this.setState({});
      this.logger.warn(`[proton] проверка обновления не удалась: ${error.message}`);
      return null;
    });
    return this.updateCheck;
  }

  async runUpdateCheck() {
    if (this.pending) await this.pending.catch(() => null);
    const selected = this.selected();
    const current = (selected && selected.recommended &&
      this.managedWineland().find((entry) => entry.path === selected.path)) || this.managedWineland()[0];
    if (!current) return null;
    const latest = await this.latestRelease();
    this.candidate = latest;
    const available = !!latest && compareWineland(latest.version, current.version) > 0;
    this.update = Object.assign({}, this.update, { checked: true, error: null,
      available, latest: latest ? latest.version.tag : current.version.tag });
    this.setState({});
    if (available) {
      this.logger.info(`[proton] доступна новая сборка ${latest.version.tag}`);
      this.emit("update-available", { from: current.version.tag, to: latest.version.tag });
    }
    return available ? latest : null;
  }

  assertSelectionIdle() {
    const selected = this.selected();
    if (selected && selected.recommended && this.inUse(selected.path)) {
      const error = new Error("Закройте игру и LiveSplit перед обновлением Proton.");
      error.code = "PROTON_IN_USE";
      throw error;
    }
  }

  async applyUpdate() {
    await this.checkForUpdate();
    if (this.pending) return this.pending;
    if (!this.update.available || !this.candidate) return null;
    this.assertSelectionIdle();
    const candidate = this.candidate;
    const selected = this.selected();
    const current = selected && selected.recommended && winelandVersion(selected.name) || this.managedWineland()[0].version;
    this.pending = (async () => {
      this.setState({ phase: "checking", percent: 0, error: null, operation: "update" });
      let installed = this.discover().find((entry) =>
        entry.recommended && winelandVersion(entry.name).tag === candidate.version.tag);
      if (installed) {
        this.replaceManagedSelection(installed);
        this.setState({ phase: "ready", percent: 100, error: null });
      } else {
        const controller = new AbortController();
        const sums = await this.readMetadata(controller, () =>
          this.readText(candidate.sums.browser_download_url, 64 * 1024, controller.signal));
        if (sums === null) return null;
        const expected = {
          name: candidate.archive.name,
          size: candidate.archive.size,
          sha256: candidate.sha256,
          sha512: parseSha512sum(sums, candidate.archive.name),
        };
        installed = await this.installArchive(candidate.archive, expected, { replaceSelection: true, controller });
      }
      if (!installed) return null;
      // Префиксы содержат абсолютные ссылки на файлы прежнего Proton.
      // Сохраняем его до подготовки префиксов и для ручного отката.
      this.update = Object.assign({}, this.update, { available: false,
        updatedFrom: current.tag, updatedTo: candidate.version.tag });
      this.setState({});
      this.emit("updated", { from: current.tag, to: candidate.version.tag, path: installed.path });
      return installed;
    })().catch((error) => {
      this.controller = null;
      this.setState({ phase: "error", error: error.message });
      throw error;
    }).finally(() => { this.pending = null; });
    return this.pending;
  }

  async ensureReference() {
    if (this.pending) return this.pending;
    const existing = this.discover().find((entry) => entry.recommended);
    if (existing) {
      this.selectReferenceIfNeeded(existing);
      return existing;
    }
    this.emit("install-started", { version: RELEASE_TAG });
    this.pending = this.downloadReference().then((installed) => {
      if (installed) this.emit("installed", { version: RELEASE_TAG, path: installed.path });
      return installed;
    }).finally(() => { this.pending = null; });
    return this.pending;
  }

  selectReferenceIfNeeded(reference) {
    const selected = this.selectedPath();
    if (!selected) {
      this.select(reference.path);
      return true;
    }
    let real = selected;
    try { real = fs.realpathSync(selected); } catch (error) { /* deleted selection */ }
    const managed = path.resolve(real).startsWith(path.resolve(this.root) + path.sep);
    const name = path.basename(real);
    if (managed && (LEGACY_CACHYOS_DIRECTORY.test(name) ||
        UPSTREAM_WINELAND_DIRECTORY.test(name))) {
      this.select(reference.path);
      this.logger.info(`[proton] выбрана HLSR-сборка Proton Wineland`);
      return true;
    }
    return false;
  }

  // Первая установка берёт закреплённую в коде сборку: её хеши проверены
  // вручную. Более новая предлагается после checkForUpdate.
  async downloadReference() {
    const controller = new AbortController();
    this.setState({ phase: "checking", percent: 0, error: null, operation: "install" });
    const archive = await this.readMetadata(controller, async () =>
      safeReleaseAsset(JSON.parse(await this.readText(RELEASE_API, undefined, controller.signal))).archive);
    if (archive === null) return null;
    return this.installArchive(archive, PINNED_ARCHIVE, { replaceSelection: false, controller });
  }

  // Метаданные читаются под тем же контроллером, что и скачивание: отмена
  // на любой стадии возвращает null и снимает занятость.
  async readMetadata(controller, read) {
    this.controller = controller;
    try {
      const value = await read();
      controller.signal.throwIfAborted();
      return value;
    } catch (error) {
      this.controller = null;
      if (controller.signal.aborted) {
        this.setState({ phase: "idle", percent: 0, error: null });
        return null;
      }
      this.setState({ phase: "error", error: error.message });
      this.logger.error(`[proton] установка не удалась: ${error.message}`);
      throw error;
    }
  }

  async installArchive(archive, expected, options) {
    const controller = (options && options.controller) || new AbortController();
    this.controller = controller;
    const signal = controller.signal;
    let staging = null;
    try {
      this.setState({ phase: "downloading", percent: 0, error: null });
      fs.mkdirSync(this.root, { recursive: true });
      staging = fs.mkdtempSync(path.join(this.root, ".download-"));
      const file = path.join(staging, archive.name);
      const response = await this.request(archive.browser_download_url, 0, signal);
      const writer = fs.createWriteStream(file, { mode: 0o600 });
      const sha256 = crypto.createHash("sha256");
      const sha512 = crypto.createHash("sha512");
      let bytes = 0;
      const meter = new Transform({ transform: (chunk, encoding, callback) => {
          bytes += chunk.length;
          if (bytes > MAX_ARCHIVE_BYTES) return callback(new Error("Архив Proton слишком велик"));
          sha256.update(chunk);
          sha512.update(chunk);
          this.setState({ percent: Math.min(99, Math.floor(bytes / expected.size * 100)) });
          callback(null, chunk);
      } });
      await pipeline(response, meter, writer, { signal });
      signal.throwIfAborted();
      if (archive.name !== expected.name || bytes !== expected.size ||
          sha256.digest("hex") !== expected.sha256 ||
          sha512.digest("hex") !== expected.sha512)
        throw new Error("Архив Proton Wineland не совпал с проверенной сборкой HLSR");

      this.setState({ phase: "extracting", percent: 100 });
      const output = path.join(staging, "extract");
      fs.mkdirSync(output);
      const expectedRoot = archive.name.replace(/\.tar\.xz$/, "");
      await listArchive(file, expectedRoot, signal);
      await run("tar", ["-xJf", file, "-C", output, "--no-same-owner", "--no-same-permissions"], signal);
      signal.throwIfAborted();
      const extracted = path.join(output, expectedRoot);
      const extractedStat = fs.lstatSync(extracted);
      if (!extractedStat.isDirectory() || extractedStat.isSymbolicLink() || !isProton(extracted))
        throw new Error("Распакованный Proton не содержит корректный исполняемый файл");
      const target = path.join(this.root, expectedRoot);
      if (!fs.existsSync(target)) fs.renameSync(extracted, target);
      const chosen = this.discover().find((entry) => entry.path === fs.realpathSync(target));
      if (!chosen) throw new Error("Установленный Proton не обнаружен");
      if (options && options.replaceSelection) this.replaceManagedSelection(chosen);
      else this.selectReferenceIfNeeded(chosen);
      this.setState({ phase: "ready", error: null });
      this.logger.info(`[proton] установлен ${chosen.name}`);
      return chosen;
    } catch (error) {
      if (signal.aborted) {
        this.setState({ phase: "idle", percent: 0, error: null });
        return null;
      }
      this.setState({ phase: "error", error: error.message });
      this.logger.error(`[proton] установка не удалась: ${error.message}`);
      throw error;
    } finally {
      if (staging) fs.rmSync(staging, { recursive: true, force: true });
      this.controller = null;
    }
  }

  // Обновление переводит на новую сборку только тех, кто сидел на сборке
  // HLSR: выбранный вручную системный Proton остаётся выбранным.
  replaceManagedSelection(reference) {
    this.assertSelectionIdle();
    const selected = this.selectedPath();
    let real = selected;
    try { real = selected && fs.realpathSync(selected); } catch (error) { /* удалён */ }
    const managed = !real || path.resolve(real).startsWith(path.resolve(this.root) + path.sep);
    if (managed) this.select(reference.path);
  }

  cancel() {
    if (!this.controller || this.controller.signal.aborted) return false;
    this.controller.abort();
    return true;
  }
}

module.exports = ProtonManager;
module.exports.safeReleaseAsset = safeReleaseAsset;
module.exports.validateArchiveListing = validateArchiveListing;
module.exports.RELEASE_TAG = RELEASE_TAG;
module.exports.RELEASES_API = RELEASES_API;
module.exports.updateCandidate = updateCandidate;
module.exports.winelandVersion = winelandVersion;
module.exports.parseSha512sum = parseSha512sum;
module.exports.PINNED_ARCHIVE = PINNED_ARCHIVE;
module.exports.WINELAND_ARCHIVE = WINELAND_ARCHIVE;
module.exports.WINELAND_DIRECTORY = WINELAND_DIRECTORY;
module.exports.LEGACY_CACHYOS_DIRECTORY = LEGACY_CACHYOS_DIRECTORY;
module.exports.UPSTREAM_WINELAND_DIRECTORY = UPSTREAM_WINELAND_DIRECTORY;
