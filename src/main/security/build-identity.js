"use strict";

const crypto = require("crypto");
const path = require("path");

// Больше куска, который нарезает scripts выпуска (MAX_CHUNK в chunks.js).
const MAX_RANGE = 256 * 1024;

// Отпечаток сборки для онлайн-функций: «<platform>/<version>/<sha256 app.asar>».
// Сервер сверяет хеш со списком официальных сборок из канала обновлений
// (Hlsr\Release\LauncherBuilds) и на чужой хеш отзывает вход. Хеш
// считается по файлу на диске один раз за запуск; app.asar читается через
// original-fs — обычный fs в Electron открывает asar как каталог.
// Сборка без app.asar (npm run dev) присылает «dev» вместо хеша.
class BuildIdentity {
  constructor(options) {
    const opts = options || {};
    this.version = opts.version;
    this.platform = opts.platform || process.platform;
    this.packaged = opts.packaged === true;
    this.resourcesPath = opts.resourcesPath || process.resourcesPath;
    this.fs = opts.fs || BuildIdentity.rawFs();
    this.pending = null;
    this.seen = null;
  }

  static rawFs() {
    try { return require("original-fs"); } catch (error) { return require("fs"); }
  }

  asarPath() {
    return path.join(this.resourcesPath, "app.asar");
  }

  hashAsar() {
    return new Promise((resolve, reject) => {
      const hash = crypto.createHash("sha256");
      try { this.seen = BuildIdentity.fingerprint(this.fs.statSync(this.asarPath())); } catch (error) { this.seen = null; }
      const stream = this.fs.createReadStream(this.asarPath());
      stream.on("data", (chunk) => hash.update(chunk));
      stream.on("error", reject);
      stream.on("end", () => resolve(hash.digest("hex")));
    });
  }

  static fingerprint(stat) {
    return `${stat.size}:${Math.trunc(stat.mtimeMs)}`;
  }

  // Считает хеш заранее: лаунчер может обновиться, пока открыт, и файл на
  // диске тогда уже не тот, что запущен. Отпечаток файла снимается до чтения.
  warm() {
    this.header().catch(() => {});
  }

  // Участок app.asar для ответа на вызов сервера (attestation.js). Если файл
  // с момента подсчёта хеша заменили (обновление при открытом лаунчере),
  // участок не отдаётся: ответ по новым байтам выдал бы целую сборку за
  // изменённую.
  async readRange(offset, length) {
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset < 0 || length <= 0 || length > MAX_RANGE)
      throw new Error("ASAR_BAD_RANGE");
    await this.header();
    if (!this.seen) throw new Error("ASAR_UNREADABLE");
    const handle = await this.fs.promises.open(this.asarPath(), "r");
    try {
      const stat = await handle.stat();
      if (BuildIdentity.fingerprint(stat) !== this.seen) throw Object.assign(new Error("ASAR_CHANGED"), { code: "ASAR_CHANGED" });
      const buffer = Buffer.alloc(length);
      const { bytesRead } = await handle.read(buffer, 0, length, offset);
      if (bytesRead !== length) throw new Error("ASAR_SHORT");
      return buffer;
    } finally {
      await handle.close();
    }
  }

  // Значение заголовка X-HLSR-Build. Без app.asar в упакованной сборке —
  // «missing»: сервер такой хеш не знает и считает сборку изменённой.
  header() {
    if (!this.pending) {
      const sha = this.packaged ? this.hashAsar().catch(() => "missing") : Promise.resolve("dev");
      this.pending = sha.then((value) => `${this.platform}/${this.version}/${value}`);
    }
    return this.pending;
  }
}

module.exports = BuildIdentity;
