"use strict";

// Модерация из админки (сервер: Hlsr\Profile\Moderation): действующие бан и
// запрет статуса и записи за 30 дней. Лаунчер показывает бан и запрет в
// профиле, а каждую новую запись — уведомлением, один раз.

const ACTIONS = Object.freeze(["ban", "unban", "mute", "unmute", "reset-status", "reset-nick", "reset-avatar",
  "restore-nick", "restore-avatar"]);
// Коды уведомлений (notifications/catalog.js) по действию.
const CODES = Object.freeze({
  ban: "MOD_BAN",
  unban: "MOD_UNBAN",
  mute: "MOD_MUTE",
  unmute: "MOD_UNMUTE",
  "reset-status": "MOD_RESET_STATUS",
  "reset-nick": "MOD_RESET_NICK",
  "reset-avatar": "MOD_RESET_AVATAR",
  "restore-nick": "MOD_RESTORE_NICK",
  "restore-avatar": "MOD_RESTORE_AVATAR",
});
const ISO = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}([+-]\d{2}:\d{2}|Z)$/;
const MAX_SEEN = 200;

function iso(value) {
  return typeof value === "string" && ISO.test(value) && !Number.isNaN(Date.parse(value)) ? value : null;
}

function reason(value) {
  // eslint-disable-next-line no-control-regex
  return typeof value === "string" ? value.replace(/[\u0000-\u001f\u007f]/g, " ").trim().slice(0, 255) : "";
}

function restriction(raw) {
  if (!raw || typeof raw !== "object" || !Number.isSafeInteger(raw.id) || raw.id <= 0) return null;
  return { id: raw.id, reason: reason(raw.reason), createdAt: iso(raw.createdAt), expiresAt: iso(raw.expiresAt) };
}

function normalize(raw) {
  const source = raw && typeof raw === "object" ? raw : {};
  const notices = (Array.isArray(source.notices) ? source.notices : []).slice(0, 50).map((item) => {
    if (!item || typeof item !== "object" || !Number.isSafeInteger(item.id) || item.id <= 0 || ACTIONS.indexOf(item.action) === -1) return null;
    return { id: item.id, action: item.action, reason: reason(item.reason), createdAt: iso(item.createdAt), expiresAt: iso(item.expiresAt) };
  }).filter(Boolean);
  return { ban: restriction(source.ban), mute: restriction(source.mute), notices };
}

// Срок вышел — ограничение больше не действует, даже без новой синхронизации.
function activeAt(item, now) {
  return !!item && (!item.expiresAt || Date.parse(item.expiresAt) > now);
}

// Новые записи, о которых ещё не было уведомления, — старые сверху.
function unseen(state, seen) {
  const known = new Set(seen);
  return state.notices.filter((item) => !known.has(item.id)).sort((a, b) => a.id - b.id);
}

function remember(seen, notices) {
  const next = seen.concat(notices.map((item) => item.id).filter((id) => seen.indexOf(id) === -1));
  return next.slice(-MAX_SEEN);
}

module.exports = { ACTIONS, CODES, normalize, activeAt, unseen, remember };
