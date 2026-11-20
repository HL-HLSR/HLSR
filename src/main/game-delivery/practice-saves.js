"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { Transform } = require("stream");
const { pipeline } = require("stream/promises");

const { DeliveryError } = require("./errors");
const { assertInside, removeWithinZone } = require("./paths");
const { languageDirs } = require("./runs-scan");
const http = require("./http-client");
const hl1PracticeSaves = require("../../../config/saves/hl1-practice-saves.manifest.json");

const MAX_ARCHIVE_BYTES = 64 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_MS = 5 * 60 * 1000;

const SAVE_NAME = /^[A-Za-z0-9_-]+\.sav$/;

// Тренировочные сейвы отрезков («load e1» … «load e62»). Недостающие
// докачиваются в SAVE игры после установки и при запуске; сейвы игрока с
// теми же именами не перезаписываются.
class PracticeSaves {
  constructor(options) {
    const opts = options || {};

    this.paths = opts.paths;
    this.archive = opts.archive || null;
    this.packs = opts.packs || [hl1PracticeSaves];
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.httpClient = opts.httpClient || http;
    this.rateLimiter = opts.rateLimiter || null;

    this.jobs = new Map();
  }

  packFor(gameId) {
    return this.packs.find((pack) => pack.gameIds.includes(String(gameId))) || null;
  }

  // Одна загрузка на игру: повторный запуск, пока сейвы качаются, ждёт её же.
  ensure(gameId, installPath) {
    const pack = this.packFor(gameId);
    if (!pack || !installPath) return Promise.resolve({ status: "not-applicable", added: 0 });

    const key = `${pack.packId}\n${path.resolve(installPath)}`;
    if (this.jobs.has(key)) return this.jobs.get(key);

    const job = this.run(pack, installPath).finally(() => this.jobs.delete(key));
    this.jobs.set(key, job);
    return job;
  }

  async run(pack, installPath) {
    const plan = planCopies(installPath, pack);
    if (!plan) return { status: "not-installed", added: 0 };
    if (!plan.length) return { status: "present", added: 0 };

    const dirs = Array.from(new Set(plan.map((copy) => path.relative(installPath, copy.dir))));
    this.logger.info(`[saves] не хватает тренировочных сейвов: ${plan.length} в ${dirs.join(", ")}`);

    // Сейвы, которые уже лежат в папке мода, копируются в языковые папки на
    // месте; архив нужен, только если какого-то файла нет нигде.
    if (plan.every((copy) => copy.source)) return this.copy(plan, installPath);

    const archivePath = path.join(this.paths.downloads, pack.archive.filename);
    const staging = path.join(this.paths.staging, `saves-${pack.packId}`);

    try {
      fs.mkdirSync(this.paths.downloads, { recursive: true });
      await this.download(pack.archive, archivePath);

      removeWithinZone(this.paths.root, staging);
      fs.mkdirSync(staging, { recursive: true });

      if (!this.archive) throw new DeliveryError("EXTRACT_FAILED", "Распаковщик не подключён");
      await this.archive.extract(archivePath, staging).promise;

      // Пока шла загрузка, игру могли удалить, а игрок — сохраниться:
      // раскладку считаем заново, папку мода заново не создаём.
      const fresh = planCopies(installPath, pack);
      if (!fresh) return { status: "not-installed", added: 0 };
      return this.copy(fresh, installPath, staging);
    } finally {
      try { fs.unlinkSync(archivePath); } catch (ignored) {}
      try { removeWithinZone(this.paths.root, staging); } catch (ignored) {}
    }
  }

  copy(plan, installPath, staging) {
    let added = 0;

    for (const copy of plan) {
      const source = copy.source || path.join(staging, copy.name);
      if (!copy.source) assertInside(staging, source);

      if (!fs.existsSync(source))
        throw new DeliveryError("EXTRACT_FAILED", `В архиве сейвов нет ${copy.name}`);

      fs.mkdirSync(copy.dir, { recursive: true });
      try {
        fs.copyFileSync(source, path.join(copy.dir, copy.name), fs.constants.COPYFILE_EXCL);
        added += 1;
      } catch (error) {
        if (error.code !== "EEXIST") throw error;
      }
    }

    const dirs = Array.from(new Set(plan.map((copy) => path.relative(installPath, copy.dir))));
    this.logger.info(`[saves] добавлено тренировочных сейвов: ${added} → ${dirs.join(", ")}`);
    return { status: "installed", added };
  }

  async download(descriptor, target) {
    let lastError = null;

    for (const provider of descriptor.providers || []) {
      try {
        await this.downloadFrom(provider.url, descriptor, target);
        return;
      } catch (error) {
        lastError = error;
        this.logger.warn(`[saves] источник ${provider.id} не сработал: ${error.message}`);
      }
    }

    throw lastError || new DeliveryError("MANIFEST_INVALID", "У архива сейвов нет ни одного источника");
  }

  async downloadFrom(url, descriptor, target) {
    if (!(descriptor.size > 0) || descriptor.size > MAX_ARCHIVE_BYTES || !/^[0-9a-f]{64}$/.test(descriptor.sha256))
      throw new DeliveryError("MANIFEST_INVALID", "У архива сейвов неверный размер или SHA-256");

    const part = `${target}.part`;

    try {
      const response = await this.httpClient.request(url, { timeout: DOWNLOAD_TIMEOUT_MS });
      if (response.statusCode !== 200) {
        response.resume();
        throw new DeliveryError("NETWORK_ERROR", `Источник ответил ${response.statusCode}`);
      }

      const hash = crypto.createHash("sha256");
      let received = 0;
      const meter = new Transform({
        transform(chunk, encoding, callback) {
          received += chunk.length;
          if (received > descriptor.size)
            return callback(new DeliveryError("SIZE_MISMATCH", "Архив сейвов длиннее объявленного"));
          hash.update(chunk);
          callback(null, chunk);
        },
      });

      const streams = [response];
      if (this.rateLimiter) streams.push(this.rateLimiter.createStream());
      streams.push(meter, fs.createWriteStream(part));
      await pipeline(...streams);

      if (received !== descriptor.size)
        throw new DeliveryError("SIZE_MISMATCH", `Размер архива сейвов не совпал: ${received} вместо ${descriptor.size}`);
      if (hash.digest("hex") !== descriptor.sha256)
        throw new DeliveryError("CHECKSUM_MISMATCH", "SHA-256 архива сейвов не совпал с объявленным");

      fs.renameSync(part, target);
    } catch (error) {
      try { fs.unlinkSync(part); } catch (ignored) {}
      throw error instanceof DeliveryError ? error : new DeliveryError("NETWORK_ERROR", error.message);
    }
  }
}

// Какие сейвы куда положить; null, если самого мода нет (игра не
// установлена). Кроме SAVE мода — SAVE языковых папок (valve_WON_russian):
// игра с языком Steam, отличным от английского, пишет сейвы туда и ищет там
// раньше, чем в папке мода. Поэтому сейв набора в языковой папке перекрыл бы
// одноимённый свой сейв игрока в папке мода — такие имена туда не кладём.
function planCopies(installPath, pack) {
  const modSave = resolveSaveDir(installPath, pack.target);
  if (!modSave) return null;

  const target = pack.target.replace(/\\/g, "/");
  const languageSaves = languageDirs(installPath, path.posix.dirname(target))
    .map((dir) => resolveSaveDir(installPath, `${dir}/${path.posix.basename(target)}`))
    .filter(Boolean)
    .map((dir) => ({ dir, present: presentSaves(dir) }));

  const inMod = presentSaves(modSave);
  const copies = [];

  for (const file of pack.files) {
    if (!SAVE_NAME.test(file.name)) continue;

    const key = file.name.toLowerCase();
    const modFile = inMod.has(key) ? path.join(modSave, inMod.get(key)) : null;
    if (!modFile) copies.push({ name: file.name, dir: modSave, source: null });

    const missing = languageSaves.filter((language) => !language.present.has(key));
    if (!missing.length) continue;
    if (modFile && !isPackFile(modFile, file)) continue;

    for (const language of missing) copies.push({ name: file.name, dir: language.dir, source: modFile });
  }

  return copies;
}

// Папка SAVE внутри мода; null, если самого мода нет (игра не установлена).
// На Linux регистр имён важен, а Wine мог создать «save»: берём существующую.
function resolveSaveDir(installPath, target) {
  const wanted = assertInside(installPath, path.join(installPath, target), "Папка сейвов");
  const parent = path.dirname(wanted);
  const name = path.basename(wanted);

  let entries;
  try {
    entries = fs.readdirSync(parent, { withFileTypes: true });
  } catch (error) {
    return null;
  }

  if (entries.some((entry) => entry.isDirectory() && entry.name === name)) return wanted;

  const other = entries.find((entry) => entry.isDirectory() && entry.name.toLowerCase() === name.toLowerCase());
  return other ? path.join(parent, other.name) : wanted;
}

// Имена в папке без учёта регистра: игра под Windows и Proton его не различает.
function presentSaves(saveDir) {
  try {
    return new Map(fs.readdirSync(saveDir).map((name) => [name.toLowerCase(), name]));
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return new Map();
  }
}

function isPackFile(filePath, file) {
  try {
    if (fs.statSync(filePath).size !== file.size) return false;
    return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex") === file.sha256;
  } catch (error) {
    return false;
  }
}

module.exports = PracticeSaves;
module.exports.SAVE_NAME = SAVE_NAME;
module.exports.planCopies = planCopies;
module.exports.resolveSaveDir = resolveSaveDir;
