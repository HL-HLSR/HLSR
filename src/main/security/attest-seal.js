"use strict";

const crypto = require("crypto");

// Ключ подписи сборки (Ed25519, 32 байта seed) вшивается в main-бандл при
// сборке. Открытой строкой в бандле его нет: seed хранится как XOR с маской,
// которую дают соль и «перец» из той же записи. Это не шифрование — ключ
// всё равно должен расшифроваться в процессе лаунчера, — а способ, чтобы
// `strings` и поиск по hex в app.asar/main.jsc ничего не находили и чтобы
// ключ пришлось доставать, разбираясь в коде. Формат общий для сборки
// (scripts/lib/attest-keys.js) и рантайма (build-key.js).
const VERSION = "1";
const LABEL = Buffer.from("hlsr-seal/1");
const SEED_BYTES = 32;

function mask(salt, pepper) {
  return crypto.createHash("sha256").update(Buffer.concat([salt, pepper, LABEL])).digest();
}

function xor(left, right) {
  const out = Buffer.alloc(left.length);
  for (let i = 0; i < left.length; i++) out[i] = left[i] ^ right[i];
  return out;
}

function seal(seed) {
  if (!Buffer.isBuffer(seed) || seed.length !== SEED_BYTES) throw new Error("seed ключа аттестации: нужно 32 байта");
  const salt = crypto.randomBytes(16);
  const pepper = crypto.randomBytes(32);
  return [VERSION, salt.toString("base64"), pepper.toString("base64"), xor(seed, mask(salt, pepper)).toString("base64")].join(".");
}

// null, если записи нет (dev, QA) или она испорчена: тогда подписывать нечем,
// и лаунчер работает как раньше — только с хешем app.asar.
function unseal(text) {
  if (typeof text !== "string" || text === "") return null;
  const parts = text.split(".");
  if (parts.length !== 4 || parts[0] !== VERSION) return null;
  const [salt, pepper, masked] = parts.slice(1).map((part) => Buffer.from(part, "base64"));
  if (salt.length !== 16 || pepper.length !== 32 || masked.length !== SEED_BYTES) return null;
  return xor(masked, mask(salt, pepper));
}

module.exports = { seal, unseal, SEED_BYTES };
