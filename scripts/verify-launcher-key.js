#!/usr/bin/env node
"use strict";

const fs = require("fs");
const crypto = require("crypto");

const { LAUNCHER_PUBLIC_KEYS } = require("../src/main/launcher-update/launcher-channel");

function keyIdOf(publicKeyPem) {
  const der = crypto.createPublicKey(publicKeyPem).export({ type: "spki", format: "der" });
  return crypto.createHash("sha256").update(der).digest("hex").slice(0, 16);
}

function spkiOf(publicKeyPem) {
  return crypto
    .createPublicKey(publicKeyPem)
    .export({ type: "spki", format: "der" })
    .toString("hex");
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--public") args.public = argv[++i];
    else if (arg === "--expect-key-id") args.expectKeyId = argv[++i];
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  return args;
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help || !args.public) {
    process.stdout.write(
      "node scripts/verify-launcher-key.js --public <файл.pub.pem> [--expect-key-id <hex16>]\n"
    );
    return args.help ? 0 : 2;
  }

  const pem = fs.readFileSync(args.public, "utf8");

  if (/PRIVATE KEY/.test(pem))
    throw new Error("передан приватный ключ; нужна только публичная половина (*.pub.pem)");

  const givenId = keyIdOf(pem);
  const givenSpki = spkiOf(pem);

  const embedded = LAUNCHER_PUBLIC_KEYS.map((k) => ({ id: keyIdOf(k), spki: spkiOf(k) }));
  const match = embedded.find((k) => k.spki === givenSpki);

  process.stdout.write(`Файл          : ${args.public}\n`);
  process.stdout.write(`keyId файла   : ${givenId}\n`);
  process.stdout.write(`Вшито в лаунчер: ${embedded.map((k) => k.id).join(", ")}\n`);

  if (args.expectKeyId && args.expectKeyId !== givenId) {
    process.stderr.write(`ОТКАЗ: ожидался keyId ${args.expectKeyId}, получен ${givenId}\n`);
    return 1;
  }

  if (!match) {
    process.stderr.write("ОТКАЗ: публичная половина НЕ совпадает с вшитым trust root\n");
    return 1;
  }

  process.stdout.write("OK: публичная половина побайтово совпадает с вшитым trust root\n");
  return 0;
}

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`Проверка не выполнена: ${error.message}\n`);
    process.exitCode = 2;
  }
}

module.exports = { keyIdOf };
