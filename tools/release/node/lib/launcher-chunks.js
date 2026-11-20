"use strict";

// Копия scripts/lib/launcher-chunks.js 0.4.7 (подпроект 6, задача 8, решения Р2, Р3, Р8) с правками.
//
// Обновление чанками при выпуске лаунчера: сборка (linux-unpacked или win-unpacked) режется в
// локальное хранилище release/launcher-chunks, рядом пишется манифест версии, а в канал идёт блок
// delta с его хешем. Хранилище общее для всех версий и платформ: на сервер уезжают только куски,
// которых там ещё нет (scripts/publish-launcher-chunks.js).
//
// Что изменилось против 0.4.7:
//  * модули чанков, подписи и адресов — замороженные копии (lib/frozen.js);
//  * аттестация — из файла ключа (lib/attest-keys.js), а не из маркера app.asar (его в 0.5 нет);
//  * buildBlock() — блок build подписанного канала (Р3): исполняемый файл с кусками и публичный
//    ключ аттестации; сервер регистрирует сборку по нему, не дожидаясь блока delta.

const fs = require("fs");
const path = require("path");
const frozen = require("./frozen");
const common = require("./release-common");
const attestKeys = require("./attest-keys");

const { chunks, endpoints } = frozen;

const STORE = path.join(common.REPO_ROOT, "release", "launcher-chunks");
const REMOTE_STORE = "launcher/chunks";

function manifestName(version, platform) {
  return `HLSR-${version}-${platform}.chunks.json`;
}

// Исполняемый файл сборки: у Windows HLSR.exe, у Linux hlsr.
function executableName(platform) {
  return platform === "linux" ? "hlsr" : "HLSR.exe";
}

// Сборка должна быть той же версии, что и канал: у Linux это .hlsr-release.json (его пишет
// hlsr-release archive), у Windows — resources/install-files.json (его пишет hlsr-release setup).
function assertTreeVersion(tree, version, platform) {
  const marker = platform === "linux" ? ".hlsr-release.json" : "resources/install-files.json";
  const file = path.join(tree, marker);
  if (!fs.existsSync(file)) throw new Error(`В сборке нет ${marker}: ${tree}`);
  const found = JSON.parse(fs.readFileSync(file, "utf8")).version;
  if (found !== version) throw new Error(`${marker} описывает ${found}, а выпускается ${version}`);
}

// Публичный ключ аттестации версии. Сервер читает его из канала (build.attest) и из манифеста
// чанков (attest); подпись канала покрывает хеш манифеста, то есть и ключ. Выпуск без аттестации
// (HLSR_ATTEST_DISABLE=1) выходит без поля.
function attestation({ version, platform, env }) {
  if (attestKeys.disabled(env, platform)) return null;
  return attestKeys.attestationFor({ platform, version, env });
}

function prepare({ version, platform, tree, outDir, store = STORE, env }) {
  const root = path.resolve(tree);
  assertTreeVersion(root, version, platform);
  const result = chunks.buildStore(root, store, { version, platform });
  const attest = attestation({ version, platform, env });
  if (attest) result.manifest.attest = attest;
  const problems = chunks.problemsOf(result.manifest, { version, platform });
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

// Блок build канала (Р3): {executable: {path, size, sha256, chunks: [[sha256, size, csize]]},
// attest: {v, publicKey}}. Куски — те же, что режет describeTree для этого файла, поэтому блок не
// зависит от того, выложены ли куски (--no-chunks). 0.4.7 незнакомое поле build пропускает, а
// подпись его покрывает.
function buildBlock({ version, platform, tree, env }) {
  const file = path.join(path.resolve(tree), executableName(platform));
  if (!fs.existsSync(file)) throw new Error(`В сборке нет ${executableName(platform)}: ${tree}`);
  const data = fs.readFileSync(file);
  const list = chunks.cut(data).map(([start, end]) => {
    const piece = data.subarray(start, end);
    return [chunks.sha256(piece), end - start, chunks.compress(piece).length];
  });
  const block = {
    executable: { path: executableName(platform), size: data.length, sha256: chunks.sha256(data), chunks: list },
  };
  const attest = attestation({ version, platform, env });
  if (attest) block.attest = attest;
  return block;
}

module.exports = { prepare, buildBlock, attestation, executableName, assertTreeVersion, manifestName, STORE, REMOTE_STORE };
