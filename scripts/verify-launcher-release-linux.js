#!/usr/bin/env node
"use strict";
const fs = require("fs");
const path = require("path");
const os = require("os");
const crypto = require("crypto");
const ArchiveInstall = require("../src/main/launcher-update/ArchiveInstall");
const channel = require("../src/main/launcher-update/launcher-channel");
const embeddedPayload = require("../src/main/launcher-update/embedded-payload");
const { CHANNEL_FILE, LATEST_YML } = require("./launcher-release-prepare-linux");

// Проверяет то же, что обновлятор: подпись канала, хеши AppImage и
// latest-linux.yml, затем ставит вшитый архив во временный каталог.
async function verify(dir, publicKeys, options = {}) {
  const manifest = JSON.parse(fs.readFileSync(path.join(dir, CHANNEL_FILE), "utf8"));
  const verdict = channel.check(manifest, { publicKeys, platform: "linux" });
  if (!verdict.ok) throw new Error(verdict.problems.join("; "));
  const yml = fs.readFileSync(path.join(dir, LATEST_YML));
  if (crypto.createHash("sha256").update(yml).digest("hex") !== manifest.latestYmlSha256)
    throw new Error(`${LATEST_YML} не совпал с подписанным каналом`);
  const image = path.join(dir, manifest.installer.filename);
  const result = await channel.verifyInstallerStream(image, manifest.installer);
  if (!result.ok) throw new Error(result.problems.join("; "));
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-release-content-"));
  const install = new ArchiveInstall(path.join(temporary, "install"));
  try {
    // Распаковываем копию: extractFrom выставляет права на исполнение.
    const copy = path.join(temporary, manifest.installer.filename);
    fs.copyFileSync(image, copy);
    const payload = await (options.extractPayload || embeddedPayload.extractFrom)(copy, temporary);
    if (payload.manifest.version !== manifest.version)
      throw new Error(`В AppImage вшит HLSR ${payload.manifest.version}, ожидался ${manifest.version}`);
    await install.prepare(payload.archive, payload.manifest);
  } finally { install.discard(); fs.rmSync(temporary, { recursive: true, force: true }); }
  return manifest;
}
if (require.main === module) verify(process.argv[2] || "build")
  .then((manifest) => console.log(`Linux-релиз ${manifest.version}: подпись, AppImage и вшитый архив проверены`))
  .catch((error) => { console.error(error.message); process.exitCode = 1; });
module.exports = { verify };
