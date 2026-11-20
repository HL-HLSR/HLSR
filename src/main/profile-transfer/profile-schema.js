"use strict";

const SCHEMA_VERSION = 1;

const FILES = {
  MANIFEST: "manifest.json",
  PLAYTIME: "playtime.json",
  PREFERENCES: "preferences.json",
  SUMMARY: "summary.json",
};

const ALLOWED_ENTRIES = [FILES.MANIFEST, FILES.PLAYTIME, FILES.PREFERENCES, FILES.SUMMARY];

const LIMITS = {
  MAX_ENTRIES: 8,
  MAX_ENTRY_BYTES: 512 * 1024,
  MAX_TOTAL_BYTES: 1024 * 1024,
  MAX_GAMES: 64,
  MAX_PLAY_SECONDS: 100 * 365 * 24 * 60 * 60,
  MAX_LAUNCH_COUNT: 10 * 1000 * 1000,
};

const GAME_ID_RE = /^[A-Za-z0-9_-]{1,32}$/;
const LANGUAGES = ["ru", "en"];

const MIN_EVENT_MS = Date.UTC(2010, 0, 1);

class ProfileError extends Error {
  constructor(code, message) {
    super(message || code);
    this.name = "ProfileError";
    this.code = code;
  }
}

const CODES = {
  DATA_INVALID: "PT_DATA_INVALID",
  SCHEMA_UNSUPPORTED: "PT_SCHEMA_UNSUPPORTED",
  MANIFEST_INVALID: "PT_MANIFEST_INVALID",
};

function isPlainObject(value) {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function integerInRange(value, min, max) {
  if (typeof value !== "number" || !Number.isInteger(value)) return null;
  if (value < min || value > max) return null;
  return value;
}

function isoDate(value, nowMs) {
  if (value == null) return null;
  if (typeof value !== "string" || value.length > 40) return undefined;

  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) return undefined;
  if (ms < MIN_EVENT_MS || ms > nowMs + 24 * 60 * 60 * 1000) return undefined;

  return new Date(ms).toISOString();
}

function normalizePlaytime(raw) {
  const games = {};
  const source = isPlainObject(raw) && isPlainObject(raw.games) ? raw.games : {};

  for (const gameId of Object.keys(source)) {
    if (!GAME_ID_RE.test(gameId)) continue;

    const game = source[gameId];
    if (!isPlainObject(game)) continue;

    const total = integerInRange(game.totalPlaySeconds, 0, LIMITS.MAX_PLAY_SECONDS);
    const launches = integerInRange(game.launchCount, 0, LIMITS.MAX_LAUNCH_COUNT);

    if (total == null || launches == null || launches === 0) continue;

    games[gameId] = {
      gameId,
      totalPlaySeconds: total,
      launchCount: launches,
      lastLaunchAt: typeof game.lastLaunchAt === "string" ? game.lastLaunchAt : null,
      lastExitAt: typeof game.lastExitAt === "string" ? game.lastExitAt : null,
      lastSessionSeconds:
        integerInRange(game.lastSessionSeconds, 0, LIMITS.MAX_PLAY_SECONDS) === null
          ? null
          : game.lastSessionSeconds,
    };

    if (Object.keys(games).length >= LIMITS.MAX_GAMES) break;
  }

  return { schemaVersion: SCHEMA_VERSION, games };
}

function normalizePreferences(raw) {
  const source = isPlainObject(raw) ? raw : {};

  return {
    schemaVersion: SCHEMA_VERSION,
    language: LANGUAGES.indexOf(source.language) !== -1 ? source.language : "ru",
    animations: source.animations !== false,
    discordRichPresence: source.discordRichPresence === true,
  };
}

function describeContents(playtime, preferences) {
  const contents = [];

  const games = Object.keys(playtime && playtime.games ? playtime.games : {});
  if (games.length) contents.push({ id: "playtime", games: games.length });
  if (preferences) contents.push({ id: "preferences" });

  return contents;
}

function checkManifest(raw, options) {
  const opts = options || {};
  const nowMs = opts.nowMs || Date.now();

  if (!isPlainObject(raw)) throw new ProfileError(CODES.MANIFEST_INVALID, "манифест не является объектом");

  if (raw.schemaVersion !== SCHEMA_VERSION)
    throw new ProfileError(
      CODES.SCHEMA_UNSUPPORTED,
      `версия формата ${raw.schemaVersion} не поддерживается (ожидается ${SCHEMA_VERSION})`
    );

  const exportedAt = isoDate(raw.exportedAt, nowMs);
  if (!exportedAt) throw new ProfileError(CODES.MANIFEST_INVALID, "некорректная дата экспорта");

  if (typeof raw.launcherVersion !== "string" || !/^\d+\.\d+\.\d+/.test(raw.launcherVersion))
    throw new ProfileError(CODES.MANIFEST_INVALID, "некорректная версия HLSR в манифесте");

  if (!isPlainObject(raw.files)) throw new ProfileError(CODES.MANIFEST_INVALID, "в манифесте нет списка файлов");

  const files = {};
  for (const name of Object.keys(raw.files)) {
    if (ALLOWED_ENTRIES.indexOf(name) === -1)
      throw new ProfileError(CODES.MANIFEST_INVALID, `неизвестный файл в манифесте: ${name}`);

    const entry = raw.files[name];
    if (!isPlainObject(entry) || typeof entry.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(entry.sha256))
      throw new ProfileError(CODES.MANIFEST_INVALID, `нет корректной контрольной суммы для ${name}`);

    const size = integerInRange(entry.size, 0, LIMITS.MAX_ENTRY_BYTES);
    if (size == null) throw new ProfileError(CODES.MANIFEST_INVALID, `некорректный размер ${name}`);

    files[name] = { size, sha256: entry.sha256.toLowerCase() };
  }

  return {
    schemaVersion: SCHEMA_VERSION,
    exportedAt,
    launcherVersion: raw.launcherVersion,
    contents: Array.isArray(raw.contents) ? raw.contents : [],
    files,
  };
}

function checkPlaytime(raw, options) {
  const opts = options || {};
  const nowMs = opts.nowMs || Date.now();

  if (!isPlainObject(raw) || raw.schemaVersion !== SCHEMA_VERSION)
    throw new ProfileError(CODES.DATA_INVALID, "некорректный формат статистики");

  if (!isPlainObject(raw.games)) throw new ProfileError(CODES.DATA_INVALID, "в статистике нет списка игр");

  const ids = Object.keys(raw.games);
  if (ids.length > LIMITS.MAX_GAMES)
    throw new ProfileError(CODES.DATA_INVALID, `слишком много игр в архиве: ${ids.length}`);

  const games = {};

  for (const gameId of ids) {
    if (!GAME_ID_RE.test(gameId))
      throw new ProfileError(CODES.DATA_INVALID, "недопустимый идентификатор игры");

    const game = raw.games[gameId];
    if (!isPlainObject(game)) throw new ProfileError(CODES.DATA_INVALID, `повреждена запись игры ${gameId}`);

    const total = integerInRange(game.totalPlaySeconds, 0, LIMITS.MAX_PLAY_SECONDS);
    if (total == null) throw new ProfileError(CODES.DATA_INVALID, `недопустимое время игры у ${gameId}`);

    const launches = integerInRange(game.launchCount, 0, LIMITS.MAX_LAUNCH_COUNT);
    if (launches == null) throw new ProfileError(CODES.DATA_INVALID, `недопустимое число запусков у ${gameId}`);

    const lastLaunchAt = isoDate(game.lastLaunchAt, nowMs);
    if (lastLaunchAt === undefined)
      throw new ProfileError(CODES.DATA_INVALID, `недопустимая дата запуска у ${gameId}`);

    const lastExitAt = isoDate(game.lastExitAt, nowMs);
    if (lastExitAt === undefined)
      throw new ProfileError(CODES.DATA_INVALID, `недопустимая дата выхода у ${gameId}`);

    let lastSessionSeconds = null;
    if (game.lastSessionSeconds != null) {
      lastSessionSeconds = integerInRange(game.lastSessionSeconds, 0, LIMITS.MAX_PLAY_SECONDS);
      if (lastSessionSeconds == null)
        throw new ProfileError(CODES.DATA_INVALID, `недопустимая длительность сессии у ${gameId}`);
    }

    games[gameId] = {
      gameId,
      totalPlaySeconds: total,
      launchCount: launches,
      lastLaunchAt,
      lastExitAt,
      lastSessionSeconds,
      currentSession: null,
    };
  }

  return { schemaVersion: SCHEMA_VERSION, games };
}

function checkPreferences(raw) {
  if (!isPlainObject(raw) || raw.schemaVersion !== SCHEMA_VERSION)
    throw new ProfileError(CODES.DATA_INVALID, "некорректный формат настроек");

  if (LANGUAGES.indexOf(raw.language) === -1)
    throw new ProfileError(CODES.DATA_INVALID, "недопустимый язык интерфейса");

  if (typeof raw.animations !== "boolean")
    throw new ProfileError(CODES.DATA_INVALID, "недопустимое значение анимаций");

  const discordRichPresence = raw.discordRichPresence === true;

  return { language: raw.language, animations: raw.animations, discordRichPresence };
}

module.exports = {
  SCHEMA_VERSION,
  FILES,
  ALLOWED_ENTRIES,
  LIMITS,
  LANGUAGES,
  CODES,
  ProfileError,
  normalizePlaytime,
  normalizePreferences,
  describeContents,
  checkManifest,
  checkPlaytime,
  checkPreferences,
};
