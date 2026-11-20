#!/usr/bin/env node
"use strict";

// Тестовые ключи для проверки выпуска (только тесты; боевые ключи не читаются и не создаются):
//   node make-keys.js <каталог вне репозитория> <версия> [платформа...]
// пишет в каталог channel.key.pem и channel.pub.pem (ключ подписи канала), test-keys.json
// ({"publicKeys": [pem]} для accept-release.js --test-keys), changelog.json (запись для версии) и
// attest/<платформа>-<версия>.json (ключи аттестации, по умолчанию win32 и linux). Печатает пути в JSON.

const fs = require("fs");
const path = require("path");
const { signing } = require("../../lib/frozen");
const attestKeys = require("../../lib/attest-keys");

const [directory, version, ...platforms] = process.argv.slice(2);
if (!directory || !version) {
  process.stderr.write("make-keys.js <каталог> <версия> [платформа...]\n");
  process.exit(2);
}

const dir = path.resolve(directory);
fs.mkdirSync(dir, { recursive: true });
const pair = signing.generateKeyPair();
const out = {
  privateKey: path.join(dir, "channel.key.pem"),
  publicKey: path.join(dir, "channel.pub.pem"),
  testKeys: path.join(dir, "test-keys.json"),
  changelog: path.join(dir, "changelog.json"),
  attestDir: path.join(dir, "attest"),
};
fs.writeFileSync(out.privateKey, pair.privateKeyPem);
fs.writeFileSync(out.publicKey, pair.publicKeyPem);
fs.writeFileSync(out.testKeys, JSON.stringify({ publicKeys: [pair.publicKeyPem] }) + "\n");
fs.writeFileSync(out.changelog, JSON.stringify({ releases: [{ version, ru: ["Проверка выпуска"], en: ["Release check"] }] }) + "\n");
const env = { HLSR_ATTEST_KEYS_DIR: out.attestDir };
for (const platform of platforms.length ? platforms : ["win32", "linux"]) attestKeys.loadOrCreate({ platform, version, env });
process.stdout.write(JSON.stringify(out) + "\n");
