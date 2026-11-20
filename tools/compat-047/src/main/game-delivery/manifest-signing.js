"use strict";

const crypto = require("crypto");
const { DeliveryError } = require("./errors");

const SIGNATURE_FIELD = "signature";
const ALGORITHM = "ed25519";

const TRUSTED_PUBLIC_KEYS = [];

function canonicalize(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);

  if (Array.isArray(value)) return `[${value.map(canonicalize).join(",")}]`;

  const keys = Object.keys(value)
    .filter((key) => key !== SIGNATURE_FIELD && value[key] !== undefined)
    .sort();

  return `{${keys.map((key) => `${JSON.stringify(key)}:${canonicalize(value[key])}`).join(",")}}`;
}

function generateKeyPair() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync(ALGORITHM);

  return {
    publicKeyPem: publicKey.export({ type: "spki", format: "pem" }).toString(),
    privateKeyPem: privateKey.export({ type: "pkcs8", format: "pem" }).toString(),
  };
}

function sign(manifest, privateKeyPem) {
  const payload = Buffer.from(canonicalize(manifest), "utf8");

  let key;
  try {
    key = crypto.createPrivateKey(privateKeyPem);
  } catch (e) {
    throw new DeliveryError("MANIFEST_INVALID", `Некорректный приватный ключ: ${e.message}`);
  }

  if (key.asymmetricKeyType !== ALGORITHM)
    throw new DeliveryError("MANIFEST_INVALID", `Ожидался ключ ${ALGORITHM}, получен ${key.asymmetricKeyType}`);

  return {
    algorithm: ALGORITHM,
    value: crypto.sign(null, payload, key).toString("base64"),
  };
}

function verify(manifest, signature, publicKeyPem) {
  if (!signature || signature.algorithm !== ALGORITHM || typeof signature.value !== "string")
    return false;

  let key;
  try {
    key = crypto.createPublicKey(publicKeyPem);
  } catch (e) {
    return false;
  }

  if (key.asymmetricKeyType !== ALGORITHM) return false;

  const payload = Buffer.from(canonicalize(manifest), "utf8");

  try {
    return crypto.verify(null, payload, key, Buffer.from(signature.value, "base64"));
  } catch (e) {
    return false;
  }
}

function check(manifest, options) {
  const opts = options || {};
  const keys = opts.publicKeys || TRUSTED_PUBLIC_KEYS;
  const signature = manifest[SIGNATURE_FIELD] || null;

  if (!signature) {
    if (opts.required)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        "Манифест без подписи отклонён: включена обязательная проверка подписи"
      );

    return { signed: false, valid: false, reason: "unsigned" };
  }

  if (!keys.length) {
    if (opts.required)
      throw new DeliveryError(
        "MANIFEST_INVALID",
        "Нет доверенных публичных ключей — подпись проверить нечем"
      );

    return { signed: true, valid: false, reason: "no-trusted-keys" };
  }

  for (const publicKeyPem of keys)
    if (verify(manifest, signature, publicKeyPem)) return { signed: true, valid: true, reason: null };

  if (opts.required)
    throw new DeliveryError("MANIFEST_INVALID", "Подпись манифеста не прошла проверку");

  return { signed: true, valid: false, reason: "bad-signature" };
}

function attach(manifest, privateKeyPem) {
  const copy = Object.assign({}, manifest);
  delete copy[SIGNATURE_FIELD];
  copy[SIGNATURE_FIELD] = sign(copy, privateKeyPem);
  return copy;
}

module.exports = {
  ALGORITHM,
  SIGNATURE_FIELD,
  TRUSTED_PUBLIC_KEYS,
  canonicalize,
  generateKeyPair,
  sign,
  verify,
  check,
  attach,
};
