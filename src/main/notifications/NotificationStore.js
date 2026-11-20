"use strict";

const { writeJsonAtomic, readJsonSafe } = require("../game-delivery/atomic");
const redact = require("../diagnostics/redact");

const FILE_VERSION = 1;
const FILE_NAME = "notifications.json";
const MAX_ITEMS = 100;
const MAX_DETAIL = 500;
const COALESCE_WINDOW_MS = 60 * 1000;

const LEVELS = ["error", "warning", "info"];
const CODE_RE = /^[A-Z0-9][A-Z0-9_.-]{0,63}$/i;

class NotificationStore {
  constructor(options) {
    const opts = options || {};
    if (!opts.file) throw new Error("NotificationStore: не задан путь к файлу");

    this.file = opts.file;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.onChange = null;

    this.seq = 0;
  }

  readAll() {
    const raw = readJsonSafe(this.file);

    if (!raw || typeof raw !== "object" || !Array.isArray(raw.items))
      return { version: FILE_VERSION, items: [] };

    const items = raw.items.filter(
      (item) =>
        item &&
        typeof item === "object" &&
        typeof item.id === "string" &&
        LEVELS.indexOf(item.level) !== -1 &&
        typeof item.ts === "number" &&
        CODE_RE.test(item.code || "")
    );

    return { version: FILE_VERSION, items };
  }

  write(all) {
    writeJsonAtomic(this.file, all);
    this.emitChange(all);
  }

  emitChange(all) {
    if (typeof this.onChange !== "function") return;
    try {
      this.onChange(this.summarize(all));
    } catch (e) {
      /* подписчик не должен ронять хранилище */
    }
  }

  summarize(all) {
    const items = (all || this.readAll()).items;
    return {
      unread: items.filter((i) => !i.read).length,
      total: items.length,
    };
  }

  list() {
    const items = this.readAll().items.slice();
    items.sort((a, b) => b.ts - a.ts);
    return {
      items,
      unread: items.filter((i) => !i.read).length,
    };
  }

  add(input) {
    const raw = input || {};

    const level = LEVELS.indexOf(raw.level) !== -1 ? raw.level : null;
    const code = CODE_RE.test(raw.code || "") ? String(raw.code) : null;

    if (!level || !code) {
      this.logger.warn(
        `[notifications] отклонено уведомление: level=${raw.level} code=${raw.code}`
      );
      return null;
    }

    const detail = redact
      .redactText(raw.detail == null ? "" : String(raw.detail))
      .slice(0, MAX_DETAIL);

    let meta = null;
    if (raw.meta && typeof raw.meta === "object") {
      meta = {};
      if (typeof raw.meta.hasDump === "boolean") meta.hasDump = raw.meta.hasDump;
      if (Number.isInteger(raw.meta.exitCode)) meta.exitCode = raw.meta.exitCode;
      if (raw.meta.hint === "DEP_ALWAYS_ON" || raw.meta.hint === "DEP_EXEMPTION_FAILED") meta.hint = raw.meta.hint;
      const VERSION_RE = /^[0-9A-Za-z.\-+]{1,32}$/;
      const GAME_ID_RE = /^[a-z0-9]+([._-][a-z0-9]+)*$/;
      if (typeof raw.meta.oldVersion === "string" && VERSION_RE.test(raw.meta.oldVersion))
        meta.oldVersion = raw.meta.oldVersion;
      if (typeof raw.meta.newVersion === "string" && VERSION_RE.test(raw.meta.newVersion))
        meta.newVersion = raw.meta.newVersion;
      if (raw.meta.target === "launcher" || raw.meta.target === "game" || raw.meta.target === "proton")
        meta.target = raw.meta.target;
      if (typeof raw.meta.hasReleaseNotes === "boolean")
        meta.hasReleaseNotes = raw.meta.hasReleaseNotes;
      if (
        typeof raw.meta.gameId === "string" &&
        raw.meta.gameId.length <= 64 &&
        GAME_ID_RE.test(raw.meta.gameId)
      )
        meta.gameId = raw.meta.gameId;
      if (typeof raw.meta.launcherId === "string" && /^[0-9]{1,8}$/.test(raw.meta.launcherId))
        meta.launcherId = raw.meta.launcherId;
      // Конец бана или запрета статуса (MOD_*); нет — навсегда.
      if (typeof raw.meta.until === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}([+-]\d{2}:\d{2}|Z)$/.test(raw.meta.until))
        meta.until = raw.meta.until;
      if (typeof raw.meta.forever === "boolean") meta.forever = raw.meta.forever;
      if (!Object.keys(meta).length) meta = null;
    }

    const all = this.readAll();
    const ts = this.now();

    const newest = all.items[all.items.length - 1];
    if (
      newest &&
      !newest.read &&
      newest.code === code &&
      newest.level === level &&
      newest.detail === detail &&
      ts - newest.ts <= COALESCE_WINDOW_MS
    ) {
      newest.count = (newest.count || 1) + 1;
      newest.ts = ts;
      this.write(all);
      return newest;
    }

    const item = {
      id: `${ts.toString(36)}-${(this.seq++).toString(36)}`,
      ts,
      level,
      code,
      detail,
      read: false,
      count: 1,
    };
    if (meta) item.meta = meta;

    all.items.push(item);

    if (all.items.length > MAX_ITEMS) all.items = all.items.slice(-MAX_ITEMS);

    this.write(all);
    return item;
  }

  removeWhere(predicate) {
    if (typeof predicate !== "function") return 0;

    const all = this.readAll();
    const kept = all.items.filter((item) => {
      try {
        return predicate(item) !== true;
      } catch (e) {
        return true;
      }
    });

    const removed = all.items.length - kept.length;
    if (!removed) return 0;

    all.items = kept;
    this.write(all);
    return removed;
  }

  markRead(id) {
    if (typeof id !== "string") return false;

    const all = this.readAll();
    const item = all.items.find((i) => i.id === id);
    if (!item || item.read) return false;

    item.read = true;
    this.write(all);
    return true;
  }

  markAllRead() {
    const all = this.readAll();
    let changed = false;

    for (const item of all.items)
      if (!item.read) {
        item.read = true;
        changed = true;
      }

    if (changed) this.write(all);
    return changed;
  }

  clear() {
    const all = this.readAll();
    if (!all.items.length) return false;

    this.write({ version: FILE_VERSION, items: [] });
    return true;
  }
}

module.exports = NotificationStore;
module.exports.FILE_NAME = FILE_NAME;
module.exports.MAX_ITEMS = MAX_ITEMS;
module.exports.LEVELS = LEVELS;
module.exports.COALESCE_WINDOW_MS = COALESCE_WINDOW_MS;
