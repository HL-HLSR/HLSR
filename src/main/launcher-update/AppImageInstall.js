"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const channel = require("./launcher-channel");

function fingerprint(file) {
  const stat = fs.lstatSync(file);
  if (!stat.isFile()) throw new Error("AppImage должен быть обычным файлом");
  return [stat.dev, stat.ino, stat.size, stat.mtimeMs].join(":");
}

function hashFile(file) {
  const hash = crypto.createHash("sha256");
  const buffer = Buffer.alloc(1024 * 1024);
  const fd = fs.openSync(file, "r");
  try {
    let count;
    while ((count = fs.readSync(fd, buffer, 0, buffer.length, null))) hash.update(buffer.subarray(0, count));
    return hash.digest("hex");
  } finally { fs.closeSync(fd); }
}

class AppImageInstall {
  constructor(target) {
    if (!target || !path.isAbsolute(target)) throw new Error("Не найден путь AppImage");
    // APPIMAGE may point to a user-created symlink; preserve that link too.
    this.target = fs.realpathSync(target);
    this.backup = `${this.target}.previous`;
    this.stage = null;
  }

  async prepare(downloaded, manifest) {
    this.discard();
    this.original = fingerprint(this.target);
    fs.accessSync(path.dirname(this.target), fs.constants.W_OK);
    const stage = path.join(path.dirname(this.target), `.hlsr-update-${crypto.randomBytes(12).toString("hex")}.AppImage`);
    try {
      await fs.promises.copyFile(downloaded, stage, fs.constants.COPYFILE_EXCL);
      const verdict = await channel.verifyInstallerStream(stage, manifest.installer);
      if (!verdict.ok) throw new Error(verdict.problems.join("; "));
      await fs.promises.chmod(stage, 0o755);
      const fd = fs.openSync(stage, "r");
      try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
      this.sha256 = manifest.installer.sha256;
      this.stage = stage;
    } catch (error) {
      try { fs.unlinkSync(stage); } catch (_) { /* absent */ }
      throw error;
    }
  }

  commit() {
    if (!this.stage) throw new Error("Нет проверенного обновления");
    if (fingerprint(this.target) !== this.original) throw new Error("AppImage изменился после скачивания обновления");
    if (hashFile(this.stage) !== this.sha256) throw new Error("SHA-256 подготовленного AppImage не совпал");
    const backupTemp = `${this.stage}.previous`;
    // A hard link keeps the original bytes without a second full copy.
    fs.linkSync(this.target, backupTemp);
    try {
      fs.renameSync(backupTemp, this.backup);
      fs.renameSync(this.stage, this.target);
      this.stage = null;
    } finally {
      try { fs.unlinkSync(backupTemp); } catch (_) { /* already renamed */ }
    }
    return this.target;
  }

  rollback() { fs.renameSync(this.backup, this.target); }

  discard() {
    if (this.stage) {
      try { fs.unlinkSync(this.stage); } catch (_) { /* already installed */ }
      this.stage = null;
    }
  }
}

module.exports = AppImageInstall;
