#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const asar = require("@electron/asar");
const common = require("./lib/release-common");
const embeddedPayload = require("../src/main/launcher-update/embedded-payload");
const { modules } = require("./build-bootstrap-app");
const root = path.resolve(__dirname, "..");
const unpacked = path.join(root, "build/bootstrap-out/linux-unpacked");
const appAsar = path.join(unpacked, "resources/app.asar");
const version = require("../package.json").version;
const artifact = path.join(root, "build", `HLSR-Setup-${version}-linux-x86_64.AppImage`);

function audit() {
  const expected = new Set(["/package.json", ...modules.map((name) => `/${name}`)]);
  const actual = asar.listPackage(appAsar).filter((name) => path.extname(name));
  const missing = [...expected].filter((name) => !actual.includes(name));
  const unexpected = actual.filter((name) => !expected.has(name));
  if (missing.length || unexpected.length)
    throw new Error(`Неверный состав bootstrap app.asar: отсутствуют ${missing.join(", ")}; лишние ${unexpected.join(", ")}`);
  for (const name of [
    "resources/installer/index.html", "resources/installer/installer.js",
    "resources/installer/preload.js", "resources/installer/style.css",
    "resources/icons/hlsr.png", "hlsr",
  ]) {
    if (!fs.statSync(path.join(unpacked, name)).isFile()) throw new Error(`В bootstrap отсутствует ${name}`);
  }
  for (const name of ["resources/umu", "resources/livesplit-fonts", "resources/livesplit-fixes", "resources/livesplit-overlay", "resources/livesplit-components", "resources/presence-relay", "resources/app.asar.unpacked"])
    if (fs.existsSync(path.join(unpacked, name))) throw new Error(`Лишние файлы лаунчера в bootstrap: ${name}`);
  if (fs.statSync(appAsar).size > 128 * 1024) throw new Error("Bootstrap app.asar подозрительно велик");
  // Встроенный лаунчер: ровно релизный архив этой версии и верное описание.
  const payload = embeddedPayload.read(path.join(unpacked, "resources", embeddedPayload.DIRECTORY));
  if (payload.manifest.version !== version) throw new Error(`Встроен HLSR ${payload.manifest.version}, ожидался ${version}`);
  const release = path.join(root, "build", embeddedPayload.archiveName(version));
  const embeddedSize = fs.statSync(payload.archive).size;
  if (embeddedSize !== payload.manifest.installer.size || embeddedSize !== fs.statSync(release).size)
    throw new Error("Размер встроенного архива не совпал с описанием или релизным архивом");
  const embeddedSha = common.sha256File(payload.archive);
  if (embeddedSha !== payload.manifest.installer.sha256 || embeddedSha !== common.sha256File(release))
    throw new Error("SHA-256 встроенного архива не совпал с описанием или релизным архивом");
  const payloadFiles = fs.readdirSync(path.dirname(payload.archive)).sort();
  if (payloadFiles.join(",") !== [embeddedPayload.DESCRIPTOR, path.basename(payload.archive)].sort().join(","))
    throw new Error(`Лишние файлы в payload: ${payloadFiles.join(", ")}`);
  if (fs.readFileSync(appAsar).toString("latin1").includes(`${root}/`))
    throw new Error("Локальный путь сборщика попал в bootstrap app.asar");
  const bytes = fs.statSync(artifact).size;
  if (bytes > embeddedSize + 125 * 1024 * 1024) throw new Error(`Bootstrap AppImage подозрительно велик: ${bytes}`);
  console.log(
    `Аудит bootstrap: OK (${(bytes / 1048576).toFixed(1)} MiB, ${actual.length} файлов в app.asar, ` +
      `встроен HLSR ${version}, ${(embeddedSize / 1048576).toFixed(1)} MiB)`
  );
}

if (require.main === module) {
  try { audit(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
module.exports = { audit };
