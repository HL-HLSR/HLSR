"use strict";

const fs = require("fs");
const crypto = require("crypto");

const signing = require("../game-delivery/manifest-signing");

// Windows и Linux подписываются одним ключом и публикуют канал одного
// формата: установщик (HLSR-Setup-<версия>.exe или
// HLSR-Setup-<версия>-linux-x86_64.AppImage), его хеши и SHA-256 latest.yml.
// Клиент принимает только установщик своей ОС.
const SUPPORTED_SCHEMA_VERSION = 1;

function channelPlatform(platform) {
  return (platform === undefined ? process.platform : platform) === "linux" ? "linux" : "win32";
}

const MAX_INSTALLER_BYTES = 512 * 1024 * 1024;

const CODES = {
  SCHEMA: "LU_SCHEMA_INVALID",
  SIGNATURE: "LU_SIGNATURE_INVALID",
  DOWNGRADE: "LU_DOWNGRADE",
  UNSUPPORTED: "LU_VERSION_UNSUPPORTED",
  VERSION_MISMATCH: "LU_VERSION_MISMATCH",
  YML: "LU_LATEST_YML_MISMATCH",
  INSTALLER_SIZE: "LU_INSTALLER_SIZE",
  INSTALLER_SHA256: "LU_INSTALLER_SHA256",
  INSTALLER_SHA512: "LU_INSTALLER_SHA512",
  INSTALLER_UNREADABLE: "LU_INSTALLER_UNREADABLE",
  INSTALLER_TOO_BIG: "LU_INSTALLER_TOO_BIG",
  NO_METADATA: "LU_NO_METADATA",
  FEED_UNAVAILABLE: "LU_FEED_UNAVAILABLE",
};

const ALLOWED_HOSTS = [
  "github.com",
  "objects.githubusercontent.com",
  "release-assets.githubusercontent.com",
  "hlsr.pro",
];

const LAUNCHER_PUBLIC_KEYS = [
  "-----BEGIN PUBLIC KEY-----\n" +
    "MCowBQYDK2VwAyEACpQQ9X1lGMQofXTJ2EuLFNU8WrYxtPR7TeyDp8bXZfY=\n" +
    "-----END PUBLIC KEY-----\n",
];

const SEMVER_RE = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;
const RELEASE_SEMVER_RE = /^\d+\.\d+\.\d+$/;
const SHA256_RE = /^[a-f0-9]{64}$/;
const SHA512_B64_RE = /^[A-Za-z0-9+/]{86}==$/;

function parseSemver(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(String(version || ""));
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] || null,
  };
}

function compareSemver(a, b) {
  const pa = parseSemver(a);
  const pb = parseSemver(b);
  if (!pa || !pb) throw new Error(`Не SemVer: ${a} / ${b}`);

  for (const part of ["major", "minor", "patch"])
    if (pa[part] !== pb[part]) return pa[part] - pb[part];

  if (pa.prerelease && !pb.prerelease) return -1;
  if (!pa.prerelease && pb.prerelease) return 1;
  if (pa.prerelease && pb.prerelease)
    return pa.prerelease < pb.prerelease ? -1 : pa.prerelease > pb.prerelease ? 1 : 0;
  return 0;
}

function isAllowedUrl(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch (e) {
    return false;
  }

  if (
    parsed.protocol !== "https:" ||
    parsed.username ||
    parsed.password ||
    (parsed.port && parsed.port !== "443")
  )
    return false;
  return ALLOWED_HOSTS.indexOf(parsed.hostname) !== -1;
}

function validateShape(data, platform) {
  const problems = [];
  const push = (msg) => problems.push(msg);
  const target = channelPlatform(platform);
  const versionRe = target === "linux" ? RELEASE_SEMVER_RE : SEMVER_RE;

  if (!data || typeof data !== "object" || Array.isArray(data))
    return ["манифест не является JSON-объектом"];

  if (data.schemaVersion !== SUPPORTED_SCHEMA_VERSION)
    push(`неподдерживаемая schemaVersion: ${data.schemaVersion}`);

  if (!versionRe.test(data.version || "")) push(`version не SemVer: ${data.version}`);
  if (typeof data.publishedAt !== "string" || isNaN(Date.parse(data.publishedAt)))
    push("publishedAt отсутствует или не дата");
  if (!versionRe.test(data.minimumSupportedVersion || ""))
    push(`minimumSupportedVersion не SemVer: ${data.minimumSupportedVersion}`);
  if (typeof data.updateRequired !== "boolean") push("updateRequired должен быть boolean");

  const installer = data.installer;
  if (!installer || typeof installer !== "object") {
    push("нет блока installer");
    return problems;
  }

  const filenameOk = target === "linux"
    ? installer.filename === `HLSR-Setup-${data.version}-linux-x86_64.AppImage`
    : typeof installer.filename === "string" && /^HLSR-Setup-.+\.exe$/.test(installer.filename);
  if (!filenameOk) push(`installer.filename неожиданный: ${installer.filename}`);
  if (!Number.isInteger(installer.size) || installer.size <= 0 || (target === "linux" && installer.size > MAX_INSTALLER_BYTES))
    push("installer.size должен быть положительным целым");
  if (!SHA256_RE.test(installer.sha256 || "")) push("installer.sha256 некорректен");
  if (!SHA512_B64_RE.test(installer.sha512 || "")) push("installer.sha512 некорректен (base64)");

  if (!Array.isArray(installer.urls) || !installer.urls.length) push("installer.urls пуст");
  else
    for (const url of installer.urls)
      if (!isAllowedUrl(url)) push(`installer.urls вне allowlist: ${url}`);

  if (data.latestYmlSha256 != null && !SHA256_RE.test(data.latestYmlSha256))
    push("latestYmlSha256 некорректен");

  return problems;
}

// Блок обновления чанками из канала, если он есть и устроен правильно.
// Подпись канала покрывает и его; без блока клиент качает установщик.
function deltaOf(data) {
  if (!data || !data.delta) return null;
  return require("./chunks").deltaProblems(data.delta, isAllowedUrl).length ? null : data.delta;
}

function check(data, options) {
  const opts = options || {};
  const currentVersion = opts.currentVersion;
  const publicKeys = opts.publicKeys || LAUNCHER_PUBLIC_KEYS;

  const problems = validateShape(data, opts.platform);

  if (!problems.length) {
    let signatureOk = false;
    for (const pem of publicKeys)
      if (signing.verify(data, data.signature, pem)) signatureOk = true;

    if (!signatureOk) problems.push("подпись Ed25519 не прошла проверку");
  }

  let updateAvailable = false;
  let downgrade = false;
  let unsupported = false;

  if (!problems.length && currentVersion) {
    const cmp = compareSemver(data.version, currentVersion);
    updateAvailable = cmp > 0;
    downgrade = cmp < 0;

    if (compareSemver(currentVersion, data.minimumSupportedVersion) < 0) {
      unsupported = true;
      problems.push(
        `установленная версия ${currentVersion} ниже минимально поддерживаемой ` +
          `${data.minimumSupportedVersion} — требуется установка вручную`
      );
    }
  }

  return {
    ok: problems.length === 0,
    problems,
    updateAvailable,
    downgrade,
    unsupported,
    code: problems.length ? (unsupported ? CODES.UNSUPPORTED : CODES.SCHEMA) : null,
  };
}

function verifyInstallerStream(filePath, installer, options) {
  const opts = options || {};
  const maxBytes = opts.maxBytes || MAX_INSTALLER_BYTES;
  const openStream = opts.createReadStream || fs.createReadStream;

  return new Promise((resolve) => {
    let stat;

    try {
      stat = (opts.statSync || fs.statSync)(filePath);
    } catch (error) {
      return resolve({
        ok: false,
        problems: [`файл не читается: ${error.message}`],
        code: CODES.INSTALLER_UNREADABLE,
      });
    }

    if (stat.size > maxBytes)
      return resolve({
        ok: false,
        problems: [`файл ${stat.size} байт больше допустимых ${maxBytes}`],
        code: CODES.INSTALLER_TOO_BIG,
      });

    if (stat.size !== installer.size)
      return resolve({
        ok: false,
        problems: [`размер ${stat.size} != ${installer.size}`],
        code: CODES.INSTALLER_SIZE,
      });

    const sha256 = crypto.createHash("sha256");
    const sha512 = crypto.createHash("sha512");
    const stream = openStream(filePath, { signal: opts.signal });

    let read = 0;
    let settled = false;

    const finish = (result) => {
      if (settled) return;
      settled = true;
      stream.destroy();
      resolve(result);
    };

    stream.on("data", (chunk) => {
      read += chunk.length;

      if (read > stat.size)
        return finish({
          ok: false,
          problems: [`файл изменился во время проверки (${read} > ${stat.size})`],
          code: CODES.INSTALLER_SIZE,
        });

      sha256.update(chunk);
      sha512.update(chunk);
    });

    stream.on("error", (error) =>
      finish({
        ok: false,
        problems: [`ошибка чтения: ${error.message}`],
        code: CODES.INSTALLER_UNREADABLE,
      })
    );

    stream.on("end", () => {
      if (read !== installer.size)
        return finish({
          ok: false,
          problems: [`прочитано ${read} из ${installer.size}`],
          code: CODES.INSTALLER_SIZE,
        });

      if (sha256.digest("hex") !== installer.sha256)
        return finish({ ok: false, problems: ["SHA-256 не совпал"], code: CODES.INSTALLER_SHA256 });

      if (sha512.digest("base64") !== installer.sha512)
        return finish({ ok: false, problems: ["SHA-512 не совпал"], code: CODES.INSTALLER_SHA512 });

      finish({ ok: true, problems: [], code: null });
    });
  });
}

module.exports = {
  SUPPORTED_SCHEMA_VERSION,
  ALLOWED_HOSTS,
  LAUNCHER_PUBLIC_KEYS,
  MAX_INSTALLER_BYTES,
  CODES,
  parseSemver,
  compareSemver,
  isAllowedUrl,
  validateShape,
  check,
  deltaOf,
  verifyInstallerStream,
};
