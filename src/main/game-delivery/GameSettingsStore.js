"use strict";

const path = require("path");

const { writeJsonAtomic, readJsonSafe } = require("./atomic");
const launchArgs = require("./launch-args");

const FILE_VERSION = 1;
const FILE_NAME = "game-settings.json";

function isSafeGameId(value) {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]{0,63}$/i.test(value);
}

class GameSettingsStore {
  constructor(options) {
    const opts = options || {};

    if (!opts.file) throw new Error("GameSettingsStore: не задан путь к файлу настроек");

    this.file = opts.file;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
  }

  readAll() {
    const raw = readJsonSafe(this.file);

    if (!raw || typeof raw !== "object" || !raw.games || typeof raw.games !== "object") {
      if (raw !== null)
        this.logger.warn(`[game-settings] ${FILE_NAME} повреждён — использую значения по умолчанию`);
      return { version: FILE_VERSION, games: {} };
    }

    return { version: FILE_VERSION, games: raw.games };
  }

  get(gameId) {
    if (!isSafeGameId(gameId)) return launchArgs.defaultSettings();

    const all = this.readAll();
    const { settings, dropped } = launchArgs.sanitizeSettings(all.games[gameId]);

    if (dropped)
      this.logger.warn(
        `[game-settings] параметры запуска ${gameId} не прошли проверку ` +
          `(${dropped.code}) — поле очищено, переключатели сохранены`
      );

    return settings;
  }

  save(gameId, rawSettings) {
    if (!isSafeGameId(gameId))
      return { ok: false, error: { code: "INVALID_GAME_ID" } };

    const normalized = launchArgs.normalizeSettings(rawSettings);
    if (!normalized.ok) return { ok: false, error: normalized.error };

    const all = this.readAll();
    all.games[gameId] = normalized.settings;

    writeJsonAtomic(this.file, all);

    this.logger.info(
      `[game-settings] сохранены настройки ${gameId}: ` +
        `аргументы [${launchArgs.buildUserArguments(normalized.settings, []).join(" ") || "нет"}]`
    );

    return { ok: true, settings: normalized.settings };
  }

  // Настройки из чужого файла: см. launchArgs.importedSettings.
  saveImported(gameId, rawSettings) {
    if (!isSafeGameId(gameId)) return { ok: false, error: { code: "INVALID_GAME_ID" } };
    return this.save(gameId, launchArgs.importedSettings(rawSettings, this.get(gameId)));
  }

  reset(gameId) {
    if (!isSafeGameId(gameId))
      return { ok: false, error: { code: "INVALID_GAME_ID" } };

    const all = this.readAll();

    if (all.games[gameId]) {
      delete all.games[gameId];
      writeJsonAtomic(this.file, all);
    }

    this.logger.info(`[game-settings] настройки ${gameId} сброшены`);
    return { ok: true, settings: launchArgs.defaultSettings() };
  }

  launchArgumentsFor(gameId, trustedArgs) {
    const settings = this.get(gameId);
    return launchArgs.buildUserArguments(settings, trustedArgs || []);
  }

  static defaultFile(userDataDir) {
    return path.join(userDataDir, FILE_NAME);
  }
}

module.exports = GameSettingsStore;
module.exports.FILE_NAME = FILE_NAME;
module.exports.FILE_VERSION = FILE_VERSION;
module.exports.isSafeGameId = isSafeGameId;
