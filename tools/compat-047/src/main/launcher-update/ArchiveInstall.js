"use strict";
const fs = require("./fs-compat");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { extract } = require("./archive");
const channel = require("./launcher-channel");
const chunks = require("./chunks");

function defaultRoot() { return path.join(os.homedir(), ".local", "opt", "hlsr"); }
function readCurrent(root) {
  try {
    const target = fs.readlinkSync(path.join(root, "current"));
    if (!/^versions\/[0-9]+\.[0-9]+\.[0-9]+-[a-f0-9]{16}$/.test(target)) throw new Error("Некорректный указатель установки HLSR");
    return target;
  } catch (error) { if (error.code === "ENOENT") return null; throw error; }
}
function managedRoot(executable) {
  try {
    const real = fs.realpathSync(executable);
    const root = path.resolve(path.dirname(real), "../..");
    const marker = JSON.parse(fs.readFileSync(path.join(root, ".hlsr-install.json"), "utf8"));
    if (marker.schema !== 1 || path.basename(path.dirname(path.dirname(real))) !== "versions") return null;
    const release = JSON.parse(fs.readFileSync(path.join(path.dirname(real), ".hlsr-release.json"), "utf8"));
    return release.platform === "linux" && release.arch === "x64" && path.basename(real) === "hlsr" ? root : null;
  } catch (_) { return null; }
}
function processIdentity(pid) {
  try {
    const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
    const start = stat.slice(stat.lastIndexOf(")") + 2).split(/\s+/)[19];
    return `${fs.readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim()}:${start}`;
  } catch (_) { return null; }
}
function switchLink(root, name, target) {
  const temp = path.join(root, `.${name}-${crypto.randomBytes(8).toString("hex")}`);
  fs.symlinkSync(target, temp);
  try { fs.renameSync(temp, path.join(root, name)); }
  finally { try { fs.unlinkSync(temp); } catch (_) { /* renamed */ } }
}

class ArchiveInstall {
  constructor(root = defaultRoot()) {
    this.root = path.resolve(root);
    this.stage = null;
    this.locked = false;
  }
  lock() {
    fs.mkdirSync(this.root, { recursive: true });
    if (fs.lstatSync(this.root).isSymbolicLink()) throw new Error("Каталог установки не должен быть ссылкой");
    const file = path.join(this.root, ".update-lock");
    const acquire = () => fs.openSync(file, "wx", 0o600);
    try { this.lockFd = acquire(); }
    catch (error) {
      if (error.code !== "EEXIST") throw error;
      let stale = false;
      const stat = fs.lstatSync(file);
      try {
        const owner = JSON.parse(fs.readFileSync(file, "utf8"));
        if (Number.isInteger(owner.pid) && owner.pid > 0 && typeof owner.identity === "string")
          stale = processIdentity(owner.pid) !== owner.identity;
      } catch (_) { stale = Date.now() - stat.mtimeMs > 30000; }
      if (!stale || fs.lstatSync(file).ino !== stat.ino) throw new Error("Другая установка уже выполняется", { cause: error });
      fs.unlinkSync(file);
      this.lockFd = acquire();
    }
    this.locked = true;
    fs.writeFileSync(this.lockFd, JSON.stringify({ pid: process.pid, identity: processIdentity(process.pid) }));
  }
  async prepare(archive, manifest, options = {}) {
    await this.stageWith(manifest, options, async (stage) => {
      const verdict = await channel.verifyInstallerStream(archive, manifest.installer, { signal: options.signal });
      if (options.signal && options.signal.aborted) throw new Error("Распаковка отменена");
      if (!verdict.ok) throw new Error(verdict.problems.join("; "));
      await extract(archive, stage, { maxBytes: manifest.installer.unpackedSize, expectedBytes: manifest.installer.unpackedSize, signal: options.signal });
    });
  }
  // Новая версия из чанков: всё, что совпало с текущей установкой, берётся
  // с диска, остальное скачивается. channelManifest — подписанный канал:
  // по его установщику называется каталог версии, как при полной загрузке.
  async prepareFromChunks(chunkManifest, channelManifest, options = {}) {
    await this.stageWith(channelManifest, options, async (stage) => {
      const source = this.original ? path.join(this.root, this.original) : null;
      this.chunkResult = await chunks.assemble({
        manifest: chunkManifest, sourceRoot: source, targetDir: stage, cacheDir: options.cacheDir,
        chunkBaseUrls: options.chunkBaseUrls, isAllowedUrl: channel.isAllowedUrl,
        fetch: options.fetch, signal: options.signal, onProgress: options.onProgress, log: options.log,
      });
    });
  }
  async stageWith(manifest, options, fill) {
    this.discard();
    this.lock();
    try {
      this.cleanOrphanStaging();
      if (options.signal && options.signal.aborted) throw new Error("Распаковка отменена");
      this.original = readCurrent(this.root);
      this.version = manifest.version;
      this.relative = `versions/${manifest.version}-${manifest.installer.sha256.slice(0, 16)}`;
      fs.mkdirSync(path.join(this.root, "versions"), { recursive: true });
      this.stage = fs.mkdtempSync(path.join(this.root, "versions", ".staging-"));
      await fill(this.stage);
      if (options.signal && options.signal.aborted) throw new Error("Распаковка отменена");
      this.verifyStage(manifest);
    } catch (error) { this.discard(); throw error; }
  }
  verifyStage(manifest) {
    const release = JSON.parse(fs.readFileSync(path.join(this.stage, ".hlsr-release.json"), "utf8"));
    if (release.version !== manifest.version || release.platform !== "linux" || release.arch !== "x64" || release.executable !== "hlsr")
      throw new Error("Версия или платформа архива не совпала с подписанным каналом");
    for (const name of ["hlsr", "resources/app.asar", "resources/icons/hlsr.png"])
      if (!fs.statSync(path.join(this.stage, name)).isFile()) throw new Error(`В архиве отсутствует ${name}`);
    const fd = fs.openSync(path.join(this.stage, "hlsr"), "r");
    const header = Buffer.alloc(20);
    try { fs.readSync(fd, header, 0, header.length, 0); } finally { fs.closeSync(fd); }
    if (header.subarray(0, 4).toString("hex") !== "7f454c46" || header[4] !== 2 || header[5] !== 1 || header.readUInt16LE(18) !== 62)
      throw new Error("В архиве нет Linux x64 бинарника");
    fs.chmodSync(path.join(this.stage, "hlsr"), 0o755);
  }
  commit() {
    if (!this.stage || !this.locked) throw new Error("Нет подготовленной версии");
    if (readCurrent(this.root) !== this.original) throw new Error("Установка HLSR изменилась во время обновления");
    const final = path.join(this.root, this.relative);
    // Never overwrite a version directory, including the one a running process uses.
    if (fs.existsSync(final)) throw new Error("Эта версия уже есть в каталоге versions");
    const script = '#!/bin/sh\nROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 1\nunset APPIMAGE APPDIR ARGV0 OWD\nexec "$ROOT/current/hlsr" --no-sandbox "$@"\n';
    const startTemp = path.join(this.root, `.start-${process.pid}`);
    fs.writeFileSync(startTemp, script, { mode: 0o755 });
    fs.renameSync(startTemp, path.join(this.root, "start"));
    fs.writeFileSync(path.join(this.root, ".hlsr-install.json"), JSON.stringify({ schema: 1 }) + "\n");
    fs.renameSync(this.stage, final);
    this.stage = null;
    try {
      if (this.original) switchLink(this.root, "previous", this.original);
      switchLink(this.root, "current", this.relative);
    } catch (error) {
      fs.rmSync(final, { recursive: true, force: true });
      throw error;
    }
    this.pruneVersions([this.relative, this.original]);
    this.unlock();
    return path.join(final, "hlsr");
  }
  rollback() {
    if (this.original) switchLink(this.root, "current", this.original);
    else fs.unlinkSync(path.join(this.root, "current"));
    fs.rmSync(path.join(this.root, this.relative), { recursive: true, force: true });
  }
  unlock() {
    if (!this.locked) return;
    fs.closeSync(this.lockFd);
    fs.unlinkSync(path.join(this.root, ".update-lock"));
    this.locked = false;
  }
  // На диске остаются только новая версия и прежняя (на неё указывает
  // previous, из неё сейчас работает лаунчер): позапрошлые удаляются.
  // Ошибка удаления не мешает обновлению.
  pruneVersions(keep) {
    const kept = new Set(keep.filter(Boolean).map((relative) => path.basename(relative)));
    let names = [];
    try { names = fs.readdirSync(path.join(this.root, "versions")); } catch (_) { return; }
    for (const name of names) {
      if (name.startsWith(".") || kept.has(name) || !/^[0-9]+\.[0-9]+\.[0-9]+-[a-f0-9]{16}$/.test(name)) continue;
      try { fs.rmSync(path.join(this.root, "versions", name), { recursive: true, force: true }); }
      catch (_) { /* удалится при следующем обновлении */ }
    }
  }
  cleanOrphanStaging() {
    // Брошенные .staging-* от упавших установок (включая эпоху бага
    // `Invalid package .../app.asar`, когда discard() сам падал на патченном
    // fs и каталог оставался). Блокировка уже наша, значит чужой живой
    // распаковки нет и удалять безопасно; текущий this.stage ещё не создан.
    try {
      const dir = path.join(this.root, "versions");
      for (const name of fs.readdirSync(dir)) {
        if (!name.startsWith(".staging-")) continue;
        const target = path.join(dir, name);
        if (this.stage && path.resolve(target) === path.resolve(this.stage)) continue;
        fs.rmSync(target, { recursive: true, force: true });
      }
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  discard() {
    if (this.stage) { fs.rmSync(this.stage, { recursive: true, force: true }); this.stage = null; }
    this.unlock();
  }
}
module.exports = ArchiveInstall;
module.exports.defaultRoot = defaultRoot;
module.exports.readCurrent = readCurrent;
module.exports.managedRoot = managedRoot;
