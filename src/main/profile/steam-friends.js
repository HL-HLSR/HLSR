"use strict";

const fs = require("fs");
const path = require("path");

const SteamAccount = require("../game-delivery/SteamAccount");

const MAX_FRIENDS = 500;

const NOT_A_FRIEND = new Set(["PersonaName", "communitypreferences", "contentdescriptorpreferences"]);

function localConfigPath(installPath, accountId) {
  return path.join(installPath, "userdata", String(accountId), "config", "localconfig.vdf");
}

// Variation selectors and Unicode tags are intentionally removed as individual code points.
// eslint-disable-next-line no-misleading-character-class
const INVISIBLE = /[\u200B-\u200F\u2060-\u206F\uFE00-\uFE0F\u{E0000}-\u{E007F}]/gu;

function personaOf(record, io) {
  const name = io.getInsensitive(record, "name");
  if (typeof name !== "string") return null;

  const visible = name.replace(INVISIBLE, "").trim();
  if (!visible) return null;

  return visible.slice(0, 64);
}

/**
 * Друзья текущего аккаунта.
 *
 * Возвращает массив { steamId64, personaName, avatarDataUri } или пустой
 * массив. Не бросает никогда: список друзей — украшение, и отсутствующий
 * или битый файл Steam не повод показывать ошибку.
 */
function list(options) {
  const opts = options || {};
  const io = opts.fs || fs;
  const vdf = opts.vdf || require("../game-delivery/vdf");
  const logger = opts.logger || { info() {}, warn() {} };

  const installPath = opts.installPath;
  const accountId = Number(opts.accountId);
  const withAvatars = opts.avatars !== false;

  if (!installPath || !Number.isInteger(accountId) || accountId <= 0) return [];

  let text;
  try {
    text = io.readFileSync(localConfigPath(installPath, accountId), "utf8");
  } catch (error) {
    return [];
  }

  let tree;
  try {
    tree = vdf.parse(text);
  } catch (error) {
    logger.warn(`[friends] localconfig.vdf не разобран: ${error.message}`);
    return [];
  }

  const store = vdf.getInsensitive(tree, "UserLocalConfigStore") || tree;
  const friends = vdf.getInsensitive(store, "friends");
  if (!friends || typeof friends !== "object") return [];

  const own = String(accountId);
  const out = [];

  for (const key of Object.keys(friends)) {
    if (out.length >= MAX_FRIENDS) break;
    if (NOT_A_FRIEND.has(key) || key === own) continue;
    if (!/^\d{1,10}$/.test(key)) continue;

    const record = friends[key];
    if (!record || typeof record !== "object") continue;

    const steamId64 = SteamAccount.steamId64Of(Number(key));

    const personaName = personaOf(record, vdf) || steamId64;

    out.push({
      steamId64,
      steamId2: SteamAccount.steamId2Of(Number(key)),
      personaName,
      avatarDataUri: withAvatars ? avatarOf(installPath, steamId64, io) : null,
    });
  }

  out.sort((a, b) => a.personaName.localeCompare(b.personaName, "ru", { sensitivity: "base" }));

  return out;
}

/**
 * Аватар из локального кеша Steam или null.
 *
 * Кеш содержит только тех, кого клиент показывал недавно, поэтому у
 * большинства друзей его не будет. Это нормально: интерфейс рисует на
 * их месте букву имени, а не пустой квадрат.
 */
function avatarOf(installPath, steamId64, io) {
  const file = path.join(installPath, "config", "avatarcache", `${steamId64}.png`);

  let stat = null;
  try {
    stat = io.statSync(file);
  } catch (error) {
    return null;
  }

  if (!stat || !stat.isFile() || stat.size <= 0 || stat.size > SteamAccount.MAX_AVATAR_BYTES) return null;

  let data;
  try {
    data = io.readFileSync(file);
  } catch (error) {
    return null;
  }

  if (!Buffer.isBuffer(data) || data.length < 8) return null;
  if (data.readUInt32BE(0) !== 0x89504e47) return null;

  return `data:image/png;base64,${data.toString("base64")}`;
}

/**
 * Аватары пачкой, асинхронно.
 *
 * Читает только запрошенные номера и только из локального кеша Steam: в
 * интернет за недостающими не ходит. Каждый файл читается через
 * fs.promises, поэтому между картинками главный процесс успевает
 * обслужить окно — в этом и весь смысл отделения от списка.
 *
 * Возвращает { steamId64: dataUri } только для тех, у кого аватар нашёлся:
 * отсутствующий кеш — обычное дело, интерфейс рисует букву имени.
 */
async function avatarsFor(installPath, ids, options) {
  const opts = options || {};
  const io = opts.fs || fs.promises;
  const out = {};

  if (!installPath || !Array.isArray(ids)) return out;

  for (const id of ids.slice(0, MAX_FRIENDS)) {
    if (!/^\d{1,20}$/.test(String(id))) continue;

    const file = path.join(installPath, "config", "avatarcache", `${id}.png`);

    let stat;
    try {
      stat = await io.stat(file);
    } catch (error) {
      continue;
    }

    if (!stat.isFile() || stat.size <= 0 || stat.size > SteamAccount.MAX_AVATAR_BYTES) continue;

    let data;
    try {
      data = await io.readFile(file);
    } catch (error) {
      continue;
    }

    if (!Buffer.isBuffer(data) || data.length < 8) continue;
    if (data.readUInt32BE(0) !== 0x89504e47) continue;

    out[String(id)] = `data:image/png;base64,${data.toString("base64")}`;
  }

  return out;
}

module.exports = { list, avatarOf, avatarsFor, localConfigPath, MAX_FRIENDS };
