"use strict";

const crypto = require("crypto");
const seal = require("./attest-seal");

// PKCS#8-обёртка над 32-байтным seed Ed25519: так его принимает Node.
const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

// Ключ подписи этой сборки. Запись вшивает webpack (DefinePlugin) только в
// production-сборку канала production; в dev, QA и тестах её нет, и
// available() ложно. Сервер знает только публичную половину — её лаунчер не
// хранит, она уезжает в манифест релиза (scripts/lib/attest-keys.js).
class BuildKey {
  constructor(sealed) {
    const seed = seal.unseal(sealed);
    this.key = seed ? crypto.createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, seed]), format: "der", type: "pkcs8" }) : null;
  }

  available() {
    return this.key !== null;
  }

  // Подпись Ed25519 (64 байта) над message.
  sign(message) {
    if (!this.key) throw new Error("ATTEST_NO_KEY");
    return crypto.sign(null, Buffer.isBuffer(message) ? message : Buffer.from(message, "utf8"), this.key);
  }
}

// Подстановка DefinePlugin: в production-сборке выражение ниже становится
// строковой константой, окружение пользователя на неё не влияет.
BuildKey.embedded = function embedded() {
  return new BuildKey(process.env.HLSR_ATTEST_SEAL);
};

module.exports = BuildKey;
