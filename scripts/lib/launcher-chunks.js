"use strict";

// Обновление чанками при выпуске лаунчера: сборка (linux-unpacked или
// win-unpacked) режется в локальное хранилище release/launcher-chunks,
// рядом пишется манифест версии, а в канал идёт блок delta с его хешем.
// Хранилище общее для всех версий и платформ: на сервер уезжают только
// куски, которых там ещё нет (scripts/publish-launcher-chunks.js).
const fs = require("fs");
const path = require("path");
const chunks = require("../../src/main/launcher-update/chunks");
const endpoints = require("../../src/shared/delivery-endpoints");
const common = require("./release-common");
const attestKeys = require("./attest-keys");

const REPO_ROOT = path.resolve(__dirname, "../..");
const STORE = path.join(REPO_ROOT, "release", "launcher-chunks");
const REMOTE_STORE = "launcher/chunks";

function manifestName(version, platform) {
  return `HLSR-${version}-${platform}.chunks.json`;
}

// Сборка должна быть той же версии, что и канал: у Linux это
// .hlsr-release.json (его пишет build-launcher-archive), у Windows —
// resources/install-files.json.
function assertTreeVersion(tree, version, platform) {
  const marker = platform === "linux" ? ".hlsr-release.json" : "resources/install-files.json";
  const file = path.join(tree, marker);
  if (!fs.existsSync(file)) throw new Error(`В сборке нет ${marker}: ${tree}`);
  const found = JSON.parse(fs.readFileSync(file, "utf8")).version;
  if (found !== version) throw new Error(`${marker} описывает ${found}, а выпускается ${version}`);
}

// Публичный ключ аттестации сборки (src/main/security/attestation.js) из
// маркера attest.json внутри app.asar. Сервер читает его отсюда же, откуда
// берёт хеш app.asar и границы кусков для вызова (Hlsr\Release\LauncherBuilds),
// а подпись канала покрывает хеш манифеста, то есть и ключ. Сборка без
// аттестации (HLSR_ATTEST_DISABLE=1) выходит без поля.
function attachAttestation(manifest, root, { version, platform }) {
  const marker = attestKeys.readMarker(path.join(root, "resources", "app.asar"));
  if (!marker) {
    if (attestKeys.disabled(process.env, platform)) return;
    throw new Error(`В resources/app.asar нет ${attestKeys.MARKER_ASAR_PATH}: сборка без ключа аттестации. ` +
      `Соберите заново (npm run build) или выпустите осознанно без аттестации: ${attestKeys.DISABLE_ENV}=1`);
  }
  const problems = attestKeys.markerProblems(marker, { version, platform });
  if (problems.length) throw new Error(`Маркер аттестации в app.asar: ${problems.join("; ")}`);
  manifest.attest = { v: 1, publicKey: marker.publicKey };
}

function prepare({ version, platform, tree, outDir, store = STORE }) {
  const root = path.resolve(tree);
  assertTreeVersion(root, version, platform);
  const result = chunks.buildStore(root, store, { version, platform });
  const problems = chunks.problemsOf(result.manifest, { version, platform });
  attachAttestation(result.manifest, root, { version, platform });
  if (problems.length) throw new Error(`Манифест чанков: ${problems.join("; ")}`);

  const filename = manifestName(version, platform);
  const manifestPath = path.join(outDir, filename);
  const body = Buffer.from(JSON.stringify(result.manifest) + "\n");
  if (fs.existsSync(manifestPath) && !fs.readFileSync(manifestPath).equals(body))
    throw new Error(`${manifestPath} уже подготовлен для другой сборки; увеличьте версию`);
  fs.writeFileSync(manifestPath, body);

  const launcher = endpoints.launcherFor(platform);
  const delta = {
    schema: chunks.SCHEMA,
    manifest: { filename, size: body.length, sha256: chunks.sha256(body), urls: launcher.chunkManifestUrls(filename) },
    chunkBaseUrls: launcher.chunkBaseUrls(),
  };
  const total = result.manifest.files.reduce((sum, file) => sum + file.size, 0);
  process.stdout.write(
    `Чанки ${platform} ${version}: ${result.manifest.files.length} файлов, ${common.formatBytesShort(total)}, ` +
    `${result.chunks} кусков (новых ${result.written}, ${common.formatBytesShort(result.writtenBytes)} сжатыми)\n`
  );
  return { delta, manifest: result.manifest, manifestPath, store };
}

module.exports = { prepare, attachAttestation, manifestName, STORE, REMOTE_STORE };
