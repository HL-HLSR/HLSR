"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");
const schema = require("./profile-schema");
const achievements = require("../profile/achievements");

const { ProfileError, FILES, LIMITS, ALLOWED_ENTRIES } = schema;

const CODES = Object.assign({}, schema.CODES, {
  BUSY: "PT_BUSY",
  BLOCKED: "PT_BLOCKED",
  NO_ARCHIVE: "PT_NO_ARCHIVE",
  ARCHIVE_UNREADABLE: "PT_ARCHIVE_UNREADABLE",
  TOO_MANY_ENTRIES: "PT_TOO_MANY_ENTRIES",
  TOO_LARGE: "PT_TOO_LARGE",
  UNSAFE_ENTRY: "PT_UNSAFE_ENTRY",
  MISSING_ENTRY: "PT_MISSING_ENTRY",
  CHECKSUM_MISMATCH: "PT_CHECKSUM_MISMATCH",
  WRITE_FAILED: "PT_WRITE_FAILED",
  EXPORT_FAILED: "PT_EXPORT_FAILED",
});

function sevenZipPath() {
  return require("../seven-zip").binPath();
}

function sha256(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex");
}

function defaultFileName(date) {
  const d = date || new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `HLSR-Profile-${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}.zip`;
}

function parseListing(stdout) {
  const entries = [];
  let current = null;
  let started = false;

  for (const rawLine of String(stdout).split(/\r?\n/)) {
    const line = rawLine.trim();

    if (!started) {
      if (/^-{5,}$/.test(line)) started = true;
      continue;
    }

    if (!line) {
      if (current && current.Path) entries.push(current);
      current = null;
      continue;
    }

    const match = /^([A-Za-z ]+) = (.*)$/.exec(line);
    if (!match) continue;

    if (match[1] === "Path") current = { Path: match[2] };
    else if (current) current[match[1]] = match[2];
  }

  if (current && current.Path) entries.push(current);
  return entries;
}

class ProfileTransfer {
  constructor(options) {
    const opts = options || {};
    if (!opts.statsFile) throw new Error("ProfileTransfer: не задан путь к статистике");

    this.statsFile = opts.statsFile;
    this.backupDir = opts.backupDir || path.join(path.dirname(opts.statsFile), "profile-backups");
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.isBlocked = typeof opts.isBlocked === "function" ? opts.isBlocked : () => null;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.sevenZip = opts.sevenZip || sevenZipPath;

    this.busy = false;
  }

  guard() {
    if (this.busy) throw new ProfileError(CODES.BUSY, "операция переноса уже выполняется");

    const reason = this.isBlocked();
    if (reason) throw new ProfileError(CODES.BLOCKED, reason);
  }

  defaultFileName(date) {
    return defaultFileName(date || new Date(this.now()));
  }

  /**
   * Читаемая сводка профиля: достижения и статистика по играм.
   *
   * Зачем отдельный файл. playtime.json — это машинный формат: словарь
   * идентификаторов и секунд. Человек, открывший архив, хочет увидеть, что
   * у него 37 часов в Ghosting и четыре достижения из шести, а не считать
   * секунды в уме.
   *
   * Достижения СЧИТАЮТСЯ здесь же тем самым модулем, который рисует их в
   * профиле, а не берутся из отдельного хранилища: хранилища у них и нет.
   * Состояние достижения всегда выводится из часов — иначе экспорт мог бы
   * увезти «выполнено», которого в статистике уже нет.
   *
   * Обратно этот файл НЕ читается: при импорте достижения пересчитаются из
   * привезённых часов сами. Файл существует для человека и для того, чтобы
   * архив можно было открыть и понять, что в нём.
   */
  buildSummary(playtime) {
    const games = (playtime && playtime.games) || {};

    const evaluated = achievements.evaluate({
      games,
      firstRunAt: (playtime && playtime.firstRunAt) || null,
    });

    const best = achievements.bestGame(games);

    const byGame = Object.keys(games)
      .map((gameId) => ({
        gameId,
        playSeconds: Math.max(0, Math.floor(Number(games[gameId].totalPlaySeconds) || 0)),
        playHours: Math.round(((Number(games[gameId].totalPlaySeconds) || 0) / 3600) * 10) / 10,
        launchCount: Math.max(0, Math.floor(Number(games[gameId].launchCount) || 0)),
      }))
      .sort((a, b) => b.playSeconds - a.playSeconds);

    const totalSeconds = byGame.reduce((sum, item) => sum + item.playSeconds, 0);

    return {
      schemaVersion: schema.SCHEMA_VERSION,
      note:
        "Справочная сводка. При импорте не используется: достижения " +
        "пересчитываются из playtime.json.",
      totals: {
        games: byGame.length,
        playSeconds: totalSeconds,
        playHours: Math.round((totalSeconds / 3600) * 10) / 10,
        launchCount: byGame.reduce((sum, item) => sum + item.launchCount, 0),
      },
      favouriteGame: best
        ? {
            gameId: best.gameId,
            playSeconds: Math.floor(best.seconds),
            playHours: Math.round((best.seconds / 3600) * 10) / 10,
          }
        : null,
      games: byGame,
      achievements: {
        unlocked: evaluated.filter((item) => item.unlocked).length,
        total: evaluated.length,
        items: evaluated.map((item) => ({
          id: item.id,
          titleKey: item.title,
          unlocked: item.unlocked,
          progress: Math.round(item.progress * 100) / 100,
          value: item.value,
          goal: item.goal,
          at: item.at || null,
        })),
      },
    };
  }

  exportProfile(options) {
    const opts = options || {};
    if (!opts.outFile) throw new ProfileError(CODES.EXPORT_FAILED, "не выбран файл экспорта");

    this.guard();
    this.busy = true;

    const temp = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-profile-"));
    const tempReal = fs.realpathSync(temp);

    try {
      const playtime = schema.normalizePlaytime(readJsonSafe(this.statsFile));
      const preferences = schema.normalizePreferences(opts.preferences);

      const payloadDir = path.join(tempReal, "payload");
      fs.mkdirSync(payloadDir, { recursive: true });

      const files = {};
      const write = (name, data) => {
        const body = JSON.stringify(data, null, 2) + "\n";
        const buffer = Buffer.from(body, "utf8");
        fs.writeFileSync(path.join(payloadDir, name), buffer);
        files[name] = { size: buffer.length, sha256: sha256(buffer) };
      };

      write(FILES.PLAYTIME, playtime);
      write(FILES.PREFERENCES, preferences);
      write(FILES.SUMMARY, this.buildSummary(playtime));

      const manifest = {
        schemaVersion: schema.SCHEMA_VERSION,
        exportedAt: new Date(this.now()).toISOString(),
        launcherVersion: this.launcherVersion,
        contents: schema.describeContents(playtime, preferences),
        files,
      };
      fs.writeFileSync(
        path.join(payloadDir, FILES.MANIFEST),
        JSON.stringify(manifest, null, 2) + "\n",
        "utf8"
      );

      const zipTemp = path.join(tempReal, "profile.zip");
      const result = spawnSync(
        this.sevenZip(),
        ["a", "-tzip", "-y", zipTemp, path.join(payloadDir, "*")],
        { windowsHide: true, timeout: 120000 }
      );

      if (result.status !== 0 || !fs.existsSync(zipTemp))
        throw new ProfileError(CODES.EXPORT_FAILED, "не удалось создать архив профиля");

      const outTmp = `${opts.outFile}.tmp`;
      fs.copyFileSync(zipTemp, outTmp);
      fs.renameSync(outTmp, opts.outFile);

      this.logger.info(
        `[profile] экспорт готов: игр ${Object.keys(playtime.games).length}, версия схемы ${schema.SCHEMA_VERSION}`
      );

      return {
        ok: true,
        fileName: path.basename(opts.outFile),
        contents: manifest.contents,
        exportedAt: manifest.exportedAt,
      };
    } finally {
      this.busy = false;
      try {
        fs.rmSync(tempReal, { recursive: true, force: true });
      } catch (e) {
        /* temp почистит ОС */
      }
    }
  }

  listArchive(file) {
    if (!file || !fs.existsSync(file)) throw new ProfileError(CODES.NO_ARCHIVE, "архив не найден");

    const listing = spawnSync(this.sevenZip(), ["l", "-slt", "-y", file], {
      windowsHide: true,
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
      timeout: 60000,
    });

    if (listing.status !== 0)
      throw new ProfileError(CODES.ARCHIVE_UNREADABLE, "архив не читается");

    const entries = parseListing(listing.stdout);

    if (!entries.length) throw new ProfileError(CODES.ARCHIVE_UNREADABLE, "архив пуст");
    if (entries.length > LIMITS.MAX_ENTRIES)
      throw new ProfileError(CODES.TOO_MANY_ENTRIES, `записей в архиве: ${entries.length}`);

    let total = 0;

    for (const entry of entries) {
      const name = entry.Path;

      if (/D/.test(entry.Attributes || "") || /[\\/]$/.test(name))
        throw new ProfileError(CODES.UNSAFE_ENTRY, "в архиве есть каталог");

      if (
        name.indexOf("..") !== -1 ||
        name.indexOf("/") !== -1 ||
        name.indexOf("\\") !== -1 ||
        /^[A-Za-z]:/.test(name) ||
        path.isAbsolute(name)
      )
        throw new ProfileError(CODES.UNSAFE_ENTRY, "недопустимое имя файла в архиве");

      if (ALLOWED_ENTRIES.indexOf(name) === -1)
        throw new ProfileError(CODES.UNSAFE_ENTRY, `в архиве посторонний файл: ${name}`);

      const size = Number(entry.Size);
      if (!Number.isFinite(size) || size < 0 || size > LIMITS.MAX_ENTRY_BYTES)
        throw new ProfileError(CODES.TOO_LARGE, `запись ${name} слишком велика`);

      total += size;
      if (total > LIMITS.MAX_TOTAL_BYTES)
        throw new ProfileError(CODES.TOO_LARGE, "распакованный размер превышает лимит");
    }

    return entries.map((entry) => ({ name: entry.Path, size: Number(entry.Size) }));
  }

  extractTo(file, dir) {
    const result = spawnSync(
      this.sevenZip(),
      ["e", "-y", `-o${dir}`, file].concat(ALLOWED_ENTRIES),
      { windowsHide: true, timeout: 120000 }
    );

    if (result.status !== 0)
      throw new ProfileError(CODES.ARCHIVE_UNREADABLE, "не удалось распаковать архив");

    const files = {};

    for (const name of ALLOWED_ENTRIES) {
      const full = path.join(dir, name);
      if (!fs.existsSync(full)) continue;

      const stat = fs.lstatSync(full);
      if (stat.isSymbolicLink() || !stat.isFile())
        throw new ProfileError(CODES.UNSAFE_ENTRY, `${name} не является обычным файлом`);

      if (stat.size > LIMITS.MAX_ENTRY_BYTES)
        throw new ProfileError(CODES.TOO_LARGE, `${name} превышает лимит размера`);

      files[name] = fs.readFileSync(full);
    }

    if (!files[FILES.MANIFEST]) throw new ProfileError(CODES.MISSING_ENTRY, "в архиве нет манифеста");

    return files;
  }

  readArchive(file) {
    this.listArchive(file);

    const temp = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-profile-in-"));
    const tempReal = fs.realpathSync(temp);

    try {
      const raw = this.extractTo(file, tempReal);

      let manifestJson;
      try {
        manifestJson = JSON.parse(raw[FILES.MANIFEST].toString("utf8"));
      } catch (e) {
        throw new ProfileError(CODES.MANIFEST_INVALID, "манифест не разбирается как JSON");
      }

      const manifest = schema.checkManifest(manifestJson, { nowMs: this.now() });

      const payload = {};

      for (const name of Object.keys(manifest.files)) {
        const buffer = raw[name];
        if (!buffer) throw new ProfileError(CODES.MISSING_ENTRY, `в архиве нет файла ${name}`);

        if (buffer.length !== manifest.files[name].size)
          throw new ProfileError(CODES.CHECKSUM_MISMATCH, `размер ${name} не совпадает с манифестом`);

        if (sha256(buffer) !== manifest.files[name].sha256)
          throw new ProfileError(CODES.CHECKSUM_MISMATCH, `контрольная сумма ${name} не совпадает`);

        try {
          payload[name] = JSON.parse(buffer.toString("utf8"));
        } catch (e) {
          throw new ProfileError(CODES.DATA_INVALID, `${name} не разбирается как JSON`);
        }
      }

      const playtime = payload[FILES.PLAYTIME]
        ? schema.checkPlaytime(payload[FILES.PLAYTIME], { nowMs: this.now() })
        : null;
      const preferences = payload[FILES.PREFERENCES]
        ? schema.checkPreferences(payload[FILES.PREFERENCES])
        : null;

      return { manifest, playtime, preferences };
    } finally {
      try {
        fs.rmSync(tempReal, { recursive: true, force: true });
      } catch (e) {
        /* temp почистит ОС */
      }
    }
  }

  inspect(file) {
    this.guard();

    const parsed = this.readArchive(file);
    const games = parsed.playtime ? Object.keys(parsed.playtime.games).length : 0;

    return {
      ok: true,
      schemaVersion: parsed.manifest.schemaVersion,
      exportedAt: parsed.manifest.exportedAt,
      launcherVersion: parsed.manifest.launcherVersion,
      contents: parsed.manifest.contents,
      games,
      hasPreferences: !!parsed.preferences,
    };
  }

  backupCurrent() {
    if (!fs.existsSync(this.statsFile)) return null;

    fs.mkdirSync(this.backupDir, { recursive: true });

    const stamp = new Date(this.now()).toISOString().replace(/[:.]/g, "-");
    const file = path.join(this.backupDir, `game-stats-${stamp}.json`);
    fs.copyFileSync(this.statsFile, file);

    return file;
  }

  importProfile(file) {
    this.guard();
    this.busy = true;

    const previous = fs.existsSync(this.statsFile) ? fs.readFileSync(this.statsFile) : null;
    let backup = null;

    try {
      const parsed = this.readArchive(file);
      backup = this.backupCurrent();

      if (parsed.playtime) {
        const next = {
          schemaVersion: parsed.playtime.schemaVersion,
          games: parsed.playtime.games,
          importedAt: new Date(this.now()).toISOString(),
        };

        try {
          writeJsonAtomic(this.statsFile, next);
        } catch (e) {
          throw new ProfileError(CODES.WRITE_FAILED, e.message);
        }
      }

      this.logger.info(
        `[profile] импорт применён: игр ${parsed.playtime ? Object.keys(parsed.playtime.games).length : 0}`
      );

      return {
        ok: true,
        backup: backup ? path.basename(backup) : null,
        games: parsed.playtime ? Object.keys(parsed.playtime.games).length : 0,
        preferences: parsed.preferences,
        exportedAt: parsed.manifest.exportedAt,
        launcherVersion: parsed.manifest.launcherVersion,
      };
    } catch (error) {
      if (previous !== null)
        try {
          fs.writeFileSync(this.statsFile, previous);
        } catch (e) {
          this.logger.error(`[profile] не удалось восстановить прежнюю статистику: ${e.message}`);
        }

      throw error;
    } finally {
      this.busy = false;
    }
  }
}

module.exports = ProfileTransfer;
module.exports.CODES = CODES;
module.exports.defaultFileName = defaultFileName;
module.exports.parseListing = parseListing;
