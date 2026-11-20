"use strict";

const path = require("path");
const { writeJsonAtomic, readJsonSafe } = require("./atomic");

const FILE_VERSION = 1;
const FILE_NAME = "game-release-notes.json";

const PARSER_VERSION = 2;

const MAX_ITEMS = 40;
const MAX_ITEM_LENGTH = 300;
const MAX_RAW_LENGTH = 2000;
const MAX_VERSIONS_PER_GAME = 10;

const MIN_SENTENCE = 25;

const VERSION_RE = /^[0-9A-Za-z.\-+]{1,32}$/;
const GAME_ID_RE = /^[a-z0-9]+([._-][a-z0-9]+)*$/;

function isVersion(value) {
  return typeof value === "string" && VERSION_RE.test(value);
}

function compareVersions(a, b) {
  const left = String(a).split(/[.\-+]/);
  const right = String(b).split(/[.\-+]/);

  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const l = parseInt(left[i], 10);
    const r = parseInt(right[i], 10);

    if (Number.isNaN(l) || Number.isNaN(r)) {
      const cmp = String(left[i] || "").localeCompare(String(right[i] || ""));
      if (cmp) return cmp;
      continue;
    }

    if (l !== r) return l - r;
  }

  return 0;
}

function isGameId(value) {
  return typeof value === "string" && value.length <= 64 && GAME_ID_RE.test(value);
}

function toBullets(line) {
  const parts = [];
  let current = "";

  for (const piece of line.split(/(?<=[.!?…])\s+(?=[«"A-ZА-ЯЁ0-9])/)) {
    if (!current) {
      current = piece;
      continue;
    }

    if (piece.length < MIN_SENTENCE && current.length + 1 + piece.length <= MAX_ITEM_LENGTH) {
      current += " " + piece;
      continue;
    }

    parts.push(current);
    current = piece;
  }

  if (current) parts.push(current);

  return parts.flatMap((part) => {
    if (part.length <= MAX_ITEM_LENGTH) return [part];

    const head = part.slice(0, MAX_ITEM_LENGTH - 1);
    const space = head.lastIndexOf(" ");
    return [(space > MAX_ITEM_LENGTH / 2 ? head.slice(0, space) : head).trim() + "…"];
  });
}

function toItems(raw) {
  if (typeof raw !== "string") return [];

  const text = raw.slice(0, MAX_RAW_LENGTH);

  return text
    .split(/\r?\n/)
    .map((line) =>
      line
        .replace(/<[^>]*>/g, " ")
        .replace(/&nbsp;/g, " ")
        .replace(/&amp;/g, "&")
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/^\s*[-*•—]\s*/, "")
        .replace(/\s+/g, " ")
        .trim()
    )
    .filter(Boolean)
    .flatMap(toBullets)
    .slice(0, MAX_ITEMS);
}

class ReleaseNotesStore {
  constructor(options) {
    const opts = options || {};
    if (!opts.dir) throw new Error("ReleaseNotesStore: не задан каталог");

    this.file = path.join(opts.dir, FILE_NAME);
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
  }

  readAll() {
    const raw = readJsonSafe(this.file);
    if (!raw || typeof raw !== "object" || raw.version !== FILE_VERSION) return { version: FILE_VERSION, games: {} };
    if (!raw.games || typeof raw.games !== "object") raw.games = {};
    return raw;
  }

  capture(gameId, version, rawNotes) {
    if (!isGameId(gameId) || !isVersion(version)) return false;

    const items = toItems(rawNotes);
    if (!items.length) return false;

    const all = this.readAll();
    if (gameId === "__proto__" || version === "__proto__") return false;

    const game = Object.prototype.hasOwnProperty.call(all.games, gameId)
      ? all.games[gameId]
      : (all.games[gameId] = {});

    const existing = Object.prototype.hasOwnProperty.call(game, version) ? game[version] : null;
    if (existing && (existing.parser || 1) >= PARSER_VERSION) return true;

    game[version] = { items, raw: String(rawNotes).slice(0, MAX_RAW_LENGTH), parser: PARSER_VERSION, ts: (existing && existing.ts) || Date.now() };

    const versions = Object.keys(game);
    if (versions.length > MAX_VERSIONS_PER_GAME) {
      versions
        .sort((a, b) => (game[a].ts || 0) - (game[b].ts || 0))
        .slice(0, versions.length - MAX_VERSIONS_PER_GAME)
        .forEach((v) => delete game[v]);
    }

    try {
      writeJsonAtomic(this.file, all);
    } catch (error) {
      this.logger.warn(`[release-notes] не сохранено: ${error.message}`);
      return false;
    }

    return true;
  }

  get(gameId, version) {
    if (!isGameId(gameId) || !isVersion(version)) return null;

    const all = this.readAll();
    const game = Object.prototype.hasOwnProperty.call(all.games, gameId)
      ? all.games[gameId]
      : null;
    if (!game || !Object.prototype.hasOwnProperty.call(game, version)) return null;

    const entry = game[version];
    if (!entry || !Array.isArray(entry.items)) return null;

    const source =
      (entry.parser || 1) < PARSER_VERSION && typeof entry.raw === "string" && entry.raw
        ? toItems(entry.raw)
        : entry.items;

    const items = source
      .filter((item) => typeof item === "string" && item.trim())
      .slice(0, MAX_ITEMS)
      .map((item) => item.slice(0, MAX_ITEM_LENGTH));

    if (!items.length) return null;

    return { gameId, version, items };
  }

  has(gameId, version) {
    return !!this.get(gameId, version);
  }

  list() {
    const all = this.readAll();
    const games = [];

    for (const gameId of Object.keys(all.games)) {
      if (!isGameId(gameId)) continue;

      const stored = all.games[gameId];
      if (!stored || typeof stored !== "object") continue;

      const versions = Object.keys(stored)
        .filter((version) => isVersion(version))
        .map((version) => {
          const entry = this.get(gameId, version);
          return entry ? { version, items: entry.items, ts: stored[version].ts || 0 } : null;
        })
        .filter(Boolean)
        .sort((a, b) => compareVersions(b.version, a.version) || b.ts - a.ts);

      if (versions.length) games.push({ gameId, versions });
    }

    return games;
  }
}

module.exports = ReleaseNotesStore;
module.exports.toItems = toItems;
module.exports.FILE_NAME = FILE_NAME;
module.exports.MAX_ITEMS = MAX_ITEMS;
module.exports.MAX_ITEM_LENGTH = MAX_ITEM_LENGTH;
