#!/usr/bin/env node
"use strict";

// verify-launcher-release-linux.js без рантайма AppImage (его нет вне Linux): вшитый архив берётся
// из payload.json и tar.gz рядом с AppImage, как их оставляет `hlsr-release archive`. Всё остальное —
// подпись, хеши AppImage и latest-linux.yml, ArchiveInstall.prepare — настоящее.
//   node verify-linux.js <каталог выпуска> <публичный ключ канала (PEM)>

const fs = require("fs");
const { verify } = require("../../verify-launcher-release-linux");
const frozen = require("../../lib/frozen");

const embedded = frozen.load("main/launcher-update/embedded-payload.js");
const [directory, publicKey] = process.argv.slice(2);

verify(directory, [fs.readFileSync(publicKey, "utf8")], { extractPayload: async () => embedded.read(directory) })
  .then((manifest) => process.stdout.write(`Linux-релиз ${manifest.version} проверен\n`))
  .catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
