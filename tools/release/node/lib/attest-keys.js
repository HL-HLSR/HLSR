"use strict";

// Копия scripts/lib/attest-keys.js 0.4.7 (подпроект 6, задача 8) с правками.
//
// Ключи аттестации сборок лаунчера: у каждой пары «платформа + версия» свой ключ Ed25519, seed лежит
// вне репозитория (HLSR_ATTEST_KEYS_DIR, по умолчанию ../HLSR-secrets/launcher-attest), публичная
// половина уходит в подписанный канал (блок build.attest) и в манифест чанков.
//
// Что изменилось против 0.4.7:
//  * app.asar в 0.5 нет — маркер из бандла (readMarker, forBuild, @electron/asar, attest-seal)
//    убран; публичная половина берётся из файла ключа;
//  * загрузка при выпуске — load(): ключ обязан уже лежать в каталоге (его создала сборка и вшила
//    seed в бинарник); ключ, созданный при подготовке канала, не совпал бы с бинарником и сервер
//    отверг бы вызов аттестации. loadOrCreate() оставлен для сборки (build.rs, ПП7) и тестов;
//  * публичная половина проверяется по seed: файл с чужой парой — отказ.

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const common = require("./release-common");

const DIR_ENV = "HLSR_ATTEST_KEYS_DIR";
const DISABLE_ENV = "HLSR_ATTEST_DISABLE";
const PLATFORMS = ["win32", "linux"];
const VERSION_RE = /^\d{1,4}\.\d{1,4}\.\d{1,6}(?:-[0-9A-Za-z.-]{1,32})?$/;
const HEX64 = /^[a-f0-9]{64}$/;
const SEED_BYTES = 32;
const PKCS8_ED25519 = Buffer.from("302e020100300506032b657004220420", "hex");

// Сборка без аттестации: явный отказ (HLSR_ATTEST_DISABLE=1) или платформа вне выпуска.
function disabled(env, platform) {
  return (env || process.env)[DISABLE_ENV] === "1" || PLATFORMS.indexOf(platform || process.platform) === -1;
}

// Каталог ключей: HLSR_ATTEST_KEYS_DIR или соседний с репозиторием HLSR-secrets/launcher-attest.
// В репозиторий, Downloads и выход сборки ключи класть нельзя — как и приватный ключ подписи каналов.
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

function publicKeyOf(seedHex) {
  const privateKey = crypto.createPrivateKey({
    key: Buffer.concat([PKCS8_ED25519, Buffer.from(seedHex, "hex")]),
    format: "der",
    type: "pkcs8",
  });
  return crypto.createPublicKey(privateKey).export({ type: "spki", format: "der" }).subarray(-32).toString("hex");
}

function generate() {
  const pair = crypto.generateKeyPairSync("ed25519");
  const seed = pair.privateKey.export({ type: "pkcs8", format: "der" }).subarray(-SEED_BYTES);
  const publicKey = pair.publicKey.export({ type: "spki", format: "der" }).subarray(-32);
  return { seed: seed.toString("hex"), publicKey: publicKey.toString("hex") };
}

function check(record, platform, version) {
  if (!record || record.platform !== platform || record.version !== version || !HEX64.test(record.seed || "") || !HEX64.test(record.publicKey || ""))
    throw new Error(`Файл ключа аттестации ${platform} ${version} повреждён`);
  if (publicKeyOf(record.seed) !== record.publicKey)
    throw new Error(`Файл ключа аттестации ${platform} ${version}: публичный ключ не получается из seed`);
  return record;
}

// Ключ выпуска. Нет файла — отказ: его создаёт сборка (loadOrCreate), а не подготовка канала.
function load({ platform, version, env }) {
  const file = fileFor(platform, version, env);
  if (!fs.existsSync(file))
    throw new Error(
      `Нет ключа аттестации ${path.basename(file)} в ${path.dirname(file)} (${DIR_ENV}). Он создаётся при сборке ` +
        `версии и вшивается в бинарник; осознанный выпуск без аттестации — ${DISABLE_ENV}=1`
    );
  return check(JSON.parse(fs.readFileSync(file, "utf8")), platform, version);
}

// Ключ сборки. Первая сборка версии создаёт его, повторные берут тот же: сервер узнаёт публичную
// половину только из манифеста релиза, и пересборка той же версии не должна менять ключ.
function loadOrCreate({ platform, version, env }) {
  const file = fileFor(platform, version, env);
  if (fs.existsSync(file)) return check(JSON.parse(fs.readFileSync(file, "utf8")), platform, version);
  const record = Object.assign({ platform, version }, generate(), { createdAt: new Date().toISOString() });
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  try {
    fs.writeFileSync(file, JSON.stringify(record, null, 2) + "\n", { mode: 0o600, flag: "wx" });
  } catch (error) {
    // Параллельная сборка успела раньше: берём её ключ.
    if (error && error.code === "EEXIST") return check(JSON.parse(fs.readFileSync(file, "utf8")), platform, version);
    throw error;
  }
  return record;
}

// Блок attest канала и манифеста чанков: {v, publicKey}.
function attestationFor({ platform, version, env }) {
  const record = load({ platform, version, env });
  return { v: 1, publicKey: record.publicKey };
}

function attestProblems(attest) {
  if (!attest || typeof attest !== "object" || attest.v !== 1) return ["аттестация не распознана"];
  if (!HEX64.test(attest.publicKey || "")) return ["публичный ключ аттестации не 32 байта hex"];
  return [];
}

module.exports = {
  DIR_ENV,
  DISABLE_ENV,
  disabled,
  keysDir,
  fileFor,
  generate,
  publicKeyOf,
  check,
  load,
  loadOrCreate,
  attestationFor,
  attestProblems,
};
