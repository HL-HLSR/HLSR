"use strict";

// Ключи аттестации сборок лаунчера (src/main/security/attestation.js).
//
// У каждой пары «платформа + версия» свой ключ Ed25519. Seed лежит вне
// репозитория, в каталоге с другими секретами выпуска, и вшивается в
// main-бандл при сборке (.electron-vue/webpack.main.config.js). Публичная
// половина попадает в app.asar маркером dist/electron/attest.json, а оттуда
// при выпуске — в манифест чанков (scripts/lib/launcher-chunks.js), который
// сервер читает по каналу обновлений. Руками ничего на сервер не переносится.
const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const common = require("./release-common");
const seal = require("../../src/main/security/attest-seal");

const DIR_ENV = "HLSR_ATTEST_KEYS_DIR";
const DISABLE_ENV = "HLSR_ATTEST_DISABLE";
const MARKER = "attest.json";
const MARKER_ASAR_PATH = "dist/electron/" + MARKER;
const PLATFORMS = ["win32", "linux"];
const VERSION_RE = /^\d{1,4}\.\d{1,4}\.\d{1,6}(?:-[0-9A-Za-z.-]{1,32})?$/;
const HEX64 = /^[a-f0-9]{64}$/;

// Сборка без аттестации: явный отказ (HLSR_ATTEST_DISABLE=1) или платформа
// вне выпуска (лаунчер выходит только под Windows и Linux).
function disabled(env, platform) {
  return (env || process.env)[DISABLE_ENV] === "1" || PLATFORMS.indexOf(platform || process.platform) === -1;
}

// Каталог ключей: HLSR_ATTEST_KEYS_DIR или соседний с репозиторием
// HLSR-secrets/launcher-attest. В репозиторий, Downloads и выход сборки
// ключи класть нельзя — как и приватный ключ подписи каналов.
function keysDir(env) {
  const source = env || process.env;
  const dir = path.resolve(source[DIR_ENV] || path.join(common.REPO_ROOT, "..", "HLSR-secrets", "launcher-attest"));
  for (const zone of common.forbiddenKeyZones())
    if (common.isInside(dir, zone))
      throw new Error(`Ключи аттестации нельзя хранить в ${zone} (получено ${dir}): они уехали бы в коммит или публикацию`);
  return dir;
}

function fileFor(platform, version, env) {
  if (PLATFORMS.indexOf(platform) === -1) throw new Error(`Неизвестная платформа: ${platform}`);
  if (!VERSION_RE.test(version)) throw new Error(`Некорректная версия: ${version}`);
  return path.join(keysDir(env), `${platform}-${version}.json`);
}

function generate() {
  const pair = crypto.generateKeyPairSync("ed25519");
  const seed = pair.privateKey.export({ type: "pkcs8", format: "der" }).subarray(-seal.SEED_BYTES);
  const publicKey = pair.publicKey.export({ type: "spki", format: "der" }).subarray(-32);
  return { seed: seed.toString("hex"), publicKey: publicKey.toString("hex") };
}

function check(record, platform, version) {
  if (!record || record.platform !== platform || record.version !== version || !HEX64.test(record.seed || "") || !HEX64.test(record.publicKey || ""))
    throw new Error(`Файл ключа аттестации ${platform} ${version} повреждён`);
  return record;
}

// Ключ сборки. Первая сборка версии создаёт его, повторные берут тот же:
// сервер узнаёт публичную половину только из манифеста релиза, и пересборка
// той же версии не должна менять ключ.
function loadOrCreate({ platform, version, env }) {
  const file = fileFor(platform, version, env);
  if (fs.existsSync(file)) return check(JSON.parse(fs.readFileSync(file, "utf8")), platform, version);
  const record = Object.assign({ platform, version }, generate(), { createdAt: new Date().toISOString() });
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  try {
    fs.writeFileSync(file, JSON.stringify(record, null, 2) + "\n", { mode: 0o600, flag: "wx" });
  } catch (error) {
    // Параллельная сборка (Windows и Linux на одной машине не бывают, но main и
    // пересборка — да) успела раньше: берём её ключ.
    if (error && error.code === "EEXIST") return check(JSON.parse(fs.readFileSync(file, "utf8")), platform, version);
    throw error;
  }
  return record;
}

// Всё, что нужно сборке: запись для DefinePlugin и маркер для app.asar.
function forBuild({ platform, version, env }) {
  const record = loadOrCreate({ platform, version, env });
  return {
    seal: seal.seal(Buffer.from(record.seed, "hex")),
    marker: { v: 1, platform, version, publicKey: record.publicKey },
  };
}

function markerProblems(marker, expected) {
  const problems = [];
  if (!marker || typeof marker !== "object" || marker.v !== 1) return ["маркер аттестации не распознан"];
  if (expected && marker.platform !== expected.platform) problems.push(`маркер для ${marker.platform}, ожидалась ${expected.platform}`);
  if (expected && marker.version !== expected.version) problems.push(`маркер для ${marker.version}, ожидалась ${expected.version}`);
  if (!HEX64.test(marker.publicKey || "")) problems.push("публичный ключ в маркере не 32 байта hex");
  return problems;
}

// Маркер из упакованного app.asar (resources/app.asar собранного лаунчера)
// или null, если сборка без аттестации. @electron/asar 4 ищет файл внутри
// архива по пути с разделителем текущей системы: на Windows «/» не находит
// ничего, поэтому путь собирается через path.join.
function readMarker(asarFile) {
  const asar = require("@electron/asar");
  let body;
  try { body = asar.extractFile(asarFile, path.join(...MARKER_ASAR_PATH.split("/"))); } catch (error) { return null; }
  return JSON.parse(body.toString("utf8"));
}

module.exports = { DIR_ENV, DISABLE_ENV, MARKER, MARKER_ASAR_PATH, disabled, keysDir, fileFor, generate, loadOrCreate, forBuild, markerProblems, readMarker };
