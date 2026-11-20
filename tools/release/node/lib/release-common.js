"use strict";

// Копия scripts/lib/release-common.js 0.4.7 (подпроект 6, задача 8, решение Р8) с правками:
//  * REPO_ROOT — корень репозитория 0.5 (tools/release/node/lib → четыре уровня вверх);
//  * убрано то, что нужно только играм (validateChannelManifest, readStructured, проверки
//    провайдеров, shared/delivery-endpoints): скрипты выпуска лаунчера его не используют;
//  * workspaceVersion() — версия выпуска из Cargo.toml (package.json в 0.5 нет), только X.Y.Z (Р9).
// Ключ канала берётся ТОЛЬКО из HLSR_MANIFEST_PRIVATE_KEY_PATH и не может лежать в репозитории,
// Downloads, build, dist, release.

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const REPO_ROOT = path.resolve(__dirname, "..", "..", "..", "..");

const PRIVATE_KEY_ENV = "HLSR_MANIFEST_PRIVATE_KEY_PATH";

function hashFile(algorithm, encoding, file) {
  const hash = crypto.createHash(algorithm);
  const fd = fs.openSync(file, "r");
  const buffer = Buffer.allocUnsafe(4 * 1024 * 1024);

  try {
    for (;;) {
      const read = fs.readSync(fd, buffer, 0, buffer.length, null);
      if (read <= 0) break;
      hash.update(buffer.subarray(0, read));
    }
  } finally {
    fs.closeSync(fd);
  }

  return hash.digest(encoding);
}

function sha256File(file) {
  return hashFile("sha256", "hex", file);
}

function sha512File(file) {
  return hashFile("sha512", "base64", file);
}

function keyIdOf(publicKeyPem) {
  const key = crypto.createPublicKey(publicKeyPem);
  const der = key.export({ type: "spki", format: "der" });
  return crypto.createHash("sha256").update(der).digest("hex").slice(0, 16);
}

function forbiddenKeyZones() {
  const home = os.homedir();
  return [
    REPO_ROOT,
    path.join(home, "Downloads"),
    path.join(REPO_ROOT, "build"),
    path.join(REPO_ROOT, "dist"),
    path.join(REPO_ROOT, "release"),
    path.join(REPO_ROOT, "target"),
  ];
}

function isInside(child, parent) {
  const rel = path.relative(path.resolve(parent), path.resolve(child));
  return rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel));
}

function assertPrivateKeyPathAllowed(keyPath) {
  if (!keyPath) throw new Error(`Не задан ${PRIVATE_KEY_ENV}`);

  if (!path.isAbsolute(keyPath))
    throw new Error(`${PRIVATE_KEY_ENV} должен быть абсолютным путём, получено: ${keyPath}`);

  for (const zone of forbiddenKeyZones())
    if (isInside(keyPath, zone))
      throw new Error(
        `Приватный ключ нельзя хранить в ${zone} (получено ${keyPath}): ` +
          "репозиторий, Downloads и выход сборки исключены, чтобы ключ не утёк в коммит или публикацию"
      );

  return path.resolve(keyPath);
}

function loadPrivateKey(env) {
  const source = env || process.env;
  const keyPath = assertPrivateKeyPathAllowed(source[PRIVATE_KEY_ENV]);

  if (!fs.existsSync(keyPath)) throw new Error(`Приватный ключ не найден: ${keyPath}`);

  return { pem: fs.readFileSync(keyPath, "utf8"), path: keyPath };
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, data) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(data, null, 2) + "\n", "utf8");
}

function formatBytesShort(bytes) {
  if (!Number.isFinite(bytes)) return "—";
  const units = ["Б", "КБ", "МБ", "ГБ"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit === 0 ? 0 : 2)} ${units[unit]}`;
}

function semverParse(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(String(version || ""));
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] || null,
  };
}

function semverCompare(a, b) {
  const pa = semverParse(a);
  const pb = semverParse(b);
  if (!pa || !pb) throw new Error(`Не SemVer: ${a} / ${b}`);

  for (const part of ["major", "minor", "patch"])
    if (pa[part] !== pb[part]) return pa[part] - pb[part];

  if (pa.prerelease && !pb.prerelease) return -1;
  if (!pa.prerelease && pb.prerelease) return 1;
  if (pa.prerelease && pb.prerelease) return pa.prerelease < pb.prerelease ? -1 : pa.prerelease > pb.prerelease ? 1 : 0;
  return 0;
}

// Версия выпуска (Р9): только X.Y.Z. Пре-релиз «0.5.0-dev» из Cargo.toml в канал не попадает:
// лаунчер с такой CARGO_PKG_VERSION счёл бы выпуск 0.5.0 новее себя и предлагал бы обновиться
// на самого себя.
function requireReleaseVersion(version) {
  if (!/^\d+\.\d+\.\d+$/.test(String(version))) {
    throw new Error(`Версия выпуска — только X.Y.Z (получено ${JSON.stringify(version)}): пре-релиз и «-dev» не публикуются`);
  }
  return version;
}

// version = "…" раздела [workspace.package] корневого Cargo.toml.
function workspaceVersion(manifest) {
  const text = fs.readFileSync(manifest || path.join(REPO_ROOT, "Cargo.toml"), "utf8");
  let inSection = false;
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) {
      inSection = trimmed === "[workspace.package]";
      continue;
    }
    const match = inSection && /^version\s*=\s*"([^"]*)"/.exec(trimmed);
    if (match) return match[1];
  }
  throw new Error("В Cargo.toml нет version в [workspace.package]");
}

module.exports = {
  REPO_ROOT,
  PRIVATE_KEY_ENV,
  sha256File,
  sha512File,
  keyIdOf,
  forbiddenKeyZones,
  isInside,
  assertPrivateKeyPathAllowed,
  loadPrivateKey,
  readJson,
  writeJson,
  formatBytesShort,
  semverParse,
  semverCompare,
  requireReleaseVersion,
  workspaceVersion,
};
