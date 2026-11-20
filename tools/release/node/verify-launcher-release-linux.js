#!/usr/bin/env node
"use strict";

// Копия scripts/verify-launcher-release-linux.js 0.4.7 (подпроект 6, задача 8) с правками:
// модули — замороженные копии tools/compat-047 (lib/frozen.js); каталог по умолчанию — выход
// `hlsr-release appimage` (target/dist/linux); --test-public-key подменяет trust root (тесты).
// Настоящий рантайм AppImage запускается только на Linux (extractFrom → --appimage-extract).

const fs = require("fs");
const path = require("path");
const os = require("os");
const crypto = require("crypto");
const frozen = require("./lib/frozen");
const common = require("./lib/release-common");
const { CHANNEL_FILE, LATEST_YML } = require("./launcher-release-prepare-linux");

const ArchiveInstall = frozen.load("main/launcher-update/ArchiveInstall.js");
const embeddedPayload = frozen.load("main/launcher-update/embedded-payload.js");
const channel = frozen.launcherChannel;

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
if (require.main === module) {
  const args = process.argv.slice(2);
  const index = args.indexOf("--test-public-key");
  let publicKeys = channel.LAUNCHER_PUBLIC_KEYS;
  if (index !== -1) {
    publicKeys = [fs.readFileSync(args[index + 1], "utf8")];
    args.splice(index, 2);
    console.log("ВНИМАНИЕ: --test-public-key подменяет trust root. Это допустимо ТОЛЬКО в тестах.");
  }
  verify(args[0] || path.join(common.REPO_ROOT, "target", "dist", "linux"), publicKeys)
    .then((manifest) => console.log(`Linux-релиз ${manifest.version}: подпись, AppImage и вшитый архив проверены`))
    .catch((error) => { console.error(error.message); process.exitCode = 1; });
}
module.exports = { verify };
