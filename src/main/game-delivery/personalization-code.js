"use strict";

const crypto = require("crypto");
const zlib = require("zlib");
const personalization = require("./ghosting-personalization");
const customPacks = require("./custom-packs");
const ghosting = require("../../shared/ghosting-settings");
const goldsrc = require("../../shared/goldsrc-settings");

const { PREFIX, MAX_CODE_LENGTH, MAX_PAYLOAD_BYTES, GAME_IDS } = require("../../shared/personalization-code");
const own = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
const object = (value) => !!value && typeof value === "object" && !Array.isArray(value);

// Игра кода → игра каталога модов (custom-packs.js).
const PACKS_GAME = { "218": "ghosting-renewed", "70": "hl1-won" };

// Моды (и всё, что появится после 0.4.8) едут после JSON пробельными символами:
// 0.4.8 отклоняет код с любым незнакомым полем (matchesClean), а пробелы после
// JSON его JSON.parse пропускает, и старый лаунчер читает всё, что знает.
// Байт — четыре символа по 2 бита.
const SPACES = [" ", "\t", "\n", "\r"];
function hide(value) {
  return [...Buffer.from(JSON.stringify(value), "utf8")].map((byte) => [6, 4, 2, 0].map((shift) => SPACES[(byte >> shift) & 3]).join("")).join("");
}
function reveal(text) {
  if (text.length % 4) fail("PERSONALIZATION_CODE_INVALID");
  const bytes = Buffer.alloc(text.length / 4);
  for (let i = 0; i < bytes.length; i++)
    for (let k = 0; k < 4; k++) bytes[i] = (bytes[i] << 2) | SPACES.indexOf(text[i * 4 + k]);
  let value;
  try { value = JSON.parse(bytes.toString("utf8")); } catch (error) { fail("PERSONALIZATION_CODE_INVALID"); }
  if (!object(value)) fail("PERSONALIZATION_CODE_INVALID");
  return value;
}

// Моды этой игры: id → включён. Незнакомые id (моды новых версий) пропускаются.
function cleanMods(id, value) {
  if (!object(value)) return null;
  const mods = {};
  for (const pack of customPacks.PACKS)
    if (pack.game === PACKS_GAME[id] && typeof value[pack.id] === "boolean") mods[pack.id] = value[pack.id];
  return Object.keys(mods).length ? mods : null;
}

class PersonalizationCodeError extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

function fail(code) { throw new PersonalizationCodeError(code); }

function gameIdOf(id) {
  if (typeof id !== "string" || !own(GAME_IDS, id)) fail("PERSONALIZATION_CODE_GAME_UNSUPPORTED");
  return GAME_IDS[id];
}

// Снимок внешнего вида, а не всех настроек игры или профиля лаунчера.
// Значения по умолчанию включены явно: импорт должен убрать и те элементы
// HUD получателя, которые у автора кода были выключены.
function snapshotLook(id, input) {
  const source = object(input) ? input : {};
  const look = { personalization: personalization.sanitizePersonalization(source.personalization) };
  if (id === "218") {
    look.personalization.extra = Object.assign(ghosting.defaults(), look.personalization.extra);
  } else {
    look.personalization.extra = {};
    Object.assign(look, goldsrc.fromFlat(goldsrc.toFlat(source.packs, source.bxt)));
  }
  return look;
}

// Порядок ключей не важен, но каждое переданное значение должно пройти
// обычную проверку настроек без подмены или отбрасывания. Новые поля с
// умолчаниями не мешают читать код предыдущей версии редактора.
function matchesClean(raw, clean) {
  if (!object(raw)) return raw === clean;
  return object(clean) && Object.keys(raw).every((key) => own(clean, key) && matchesClean(raw[key], clean[key]));
}

function checksum(id, bytes) {
  return crypto.createHash("sha256").update(`${PREFIX}.${id}.`).update(bytes).digest("hex").slice(0, 16);
}

// input.mods — моды игры (id → включён), в код идут после JSON (hide).
function encode(id, input) {
  const gameId = gameIdOf(id);
  const mods = cleanMods(gameId, object(input) && input.mods);
  const bytes = Buffer.from(JSON.stringify(snapshotLook(gameId, input)) + (mods ? hide({ mods }) : ""), "utf8");
  if (bytes.length > MAX_PAYLOAD_BYTES) fail("PERSONALIZATION_CODE_TOO_LARGE");
  const packed = zlib.deflateRawSync(bytes, { level: 9 });
  const code = `${PREFIX}.${gameId}.${packed.toString("base64url")}.${checksum(gameId, packed)}`;
  if (code.length > MAX_CODE_LENGTH) fail("PERSONALIZATION_CODE_TOO_LARGE");
  return code;
}

function decode(id, input) {
  const gameId = gameIdOf(id);
  if (typeof input !== "string" || !input.trim()) fail("PERSONALIZATION_CODE_INVALID");
  if (input.length > MAX_CODE_LENGTH) fail("PERSONALIZATION_CODE_TOO_LARGE");
  // Код можно вставить с переносами строк из сообщения.
  const code = input.replace(/\s/g, "");
  const parts = code.split(".");
  if (parts.length !== 4 || !/^HLSR-P\d+$/.test(parts[0])) fail("PERSONALIZATION_CODE_INVALID");
  if (parts[0] !== PREFIX) fail("PERSONALIZATION_CODE_VERSION_UNSUPPORTED");
  if (!own(GAME_IDS, parts[1]) || GAME_IDS[parts[1]] !== parts[1]) fail("PERSONALIZATION_CODE_INVALID");
  if (parts[1] !== gameId) fail("PERSONALIZATION_CODE_WRONG_GAME");
  if (!/^[A-Za-z0-9_-]+$/.test(parts[2]) || !/^[a-f0-9]{16}$/.test(parts[3])) fail("PERSONALIZATION_CODE_INVALID");
  const packed = Buffer.from(parts[2], "base64url");
  if (packed.toString("base64url") !== parts[2] || checksum(gameId, packed) !== parts[3]) fail("PERSONALIZATION_CODE_INVALID");

  let raw, text;
  try {
    const inflated = zlib.inflateRawSync(packed, { maxOutputLength: MAX_PAYLOAD_BYTES, info: true });
    if (inflated.engine.bytesWritten !== packed.length) fail("PERSONALIZATION_CODE_INVALID");
    text = inflated.buffer.toString("utf8");
    raw = JSON.parse(text);
  } catch (error) {
    if (error.code === "ERR_BUFFER_TOO_LARGE") fail("PERSONALIZATION_CODE_TOO_LARGE");
    fail("PERSONALIZATION_CODE_INVALID");
  }
  if (!object(raw) || !object(raw.personalization) || !object(raw.personalization.extra)) fail("PERSONALIZATION_CODE_INVALID");
  const look = snapshotLook(gameId, raw);
  if (!matchesClean(raw, look)) fail("PERSONALIZATION_CODE_INVALID");
  // Чужой код не указывает на файлы этого компьютера: спрей лаунчера — только свой, по хешу.
  const spray = look.personalization.extra.gh_spray_file;
  if (spray && !ghosting.isManagedSpray(spray)) look.personalization.extra.gh_spray_file = "";
  // После JSON — только пробельные символы (их пропустил JSON.parse): там моды.
  const after = text.slice(text.lastIndexOf("}") + 1);
  const mods = after ? cleanMods(gameId, reveal(after).mods) : null;
  if (mods) look.mods = mods;
  return look;
}

module.exports = { encode, decode, MAX_CODE_LENGTH, MAX_PAYLOAD_BYTES, PersonalizationCodeError };
