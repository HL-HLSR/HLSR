"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const REPO_ROOT = path.resolve(__dirname, "..", "..");

const endpoints = require(path.join(REPO_ROOT, "src", "shared", "delivery-endpoints"));

const PRIVATE_KEY_ENV = "HLSR_MANIFEST_PRIVATE_KEY_PATH";

const ALLOWED_HTTPS_HOSTS = endpoints.ALLOWED_DOWNLOAD_HOSTS;
const ALLOWED_YANDEX_RE = /^https:\/\/disk\.yandex\.(ru|com)\/d\/[A-Za-z0-9_-]+$/;

function sha256File(file) {
  const hash = crypto.createHash("sha256");
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

  return hash.digest("hex");
}

function sha512File(file) {
  const hash = crypto.createHash("sha512");
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

  return hash.digest("base64");
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
          "worktree, Downloads и build-выход исключены, чтобы ключ не утёк в коммит или публикацию"
      );

  return path.resolve(keyPath);
}

function loadPrivateKey(env) {
  const source = env || process.env;
  const keyPath = assertPrivateKeyPathAllowed(source[PRIVATE_KEY_ENV]);

  if (!fs.existsSync(keyPath))
    throw new Error(`Приватный ключ не найден: ${keyPath}. Сначала npm run game:channel:keygen`);

  return { pem: fs.readFileSync(keyPath, "utf8"), path: keyPath };
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

const BINARY_EXTENSIONS = new Set([".7z", ".exe", ".blockmap", ".zip", ".dll", ".bin", ".dmp"]);

const STRUCTURED_PARSERS = {
  ".json": (text) => JSON.parse(text),
  ".yml": (text) => require("js-yaml").load(text),
  ".yaml": (text) => require("js-yaml").load(text),
};

function readStructured(file) {
  const ext = path.extname(file).toLowerCase();

  if (BINARY_EXTENSIONS.has(ext))
    throw new Error(
      `Бинарный артефакт ${path.basename(file)} не разбирается как текст. ` +
        "Структурная проверка применима только к .json, .yml и .yaml."
    );

  const parse = STRUCTURED_PARSERS[ext];
  if (!parse)
    throw new Error(
      `Неизвестный тип артефакта для структурной проверки: ${path.basename(file)}. ` +
        "Добавьте парсер в release-common.readStructured или уберите артефакт из фазы manifest."
    );

  const text = fs.readFileSync(file, "utf8");
  let data;
  try {
    data = parse(text);
  } catch (error) {
    throw new Error(`${path.basename(file)}: не разбирается как ${ext.slice(1).toUpperCase()}: ${error.message}`);
  }

  if (data === null || typeof data !== "object")
    throw new Error(`${path.basename(file)}: ожидался объект, получено ${data === null ? "null" : typeof data}`);

  return data;
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

function formatBytes(bytes) {
  if (!Number.isFinite(bytes)) return "—";
  return `${formatBytesShort(bytes)} (${bytes.toLocaleString("ru-RU")})`;
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

function checkProviderUrls(providers, label, problems) {
  for (const provider of providers || []) {
    if (provider.type === "https-direct") {
      let host = null;
      try {
        host = new URL(provider.url).hostname;
      } catch (e) {
        problems.push(`${label}: провайдер ${provider.id} — некорректный URL ${provider.url}`);
        continue;
      }
      if (!endpoints.isAllowedDownloadUrl(provider.url))
        problems.push(
          `${label}: провайдер ${provider.id} указывает на ${host}, ` +
            `разрешены только: ${ALLOWED_HTTPS_HOSTS.join(", ")}`
        );
    } else if (provider.type === "yandex-public") {
      if (!ALLOWED_YANDEX_RE.test(provider.publicUrl))
        problems.push(`${label}: провайдер ${provider.id} — недопустимый publicUrl`);
    } else if (provider.type === "local-http") {
      problems.push(`${label}: провайдер ${provider.id} типа local-http не подлежит подписи для публикации`);
    }
  }
}

function validateChannelManifest(data, options) {
  const opts = options || {};
  const problems = [];

  const { compileSchema } = require(path.join(
    REPO_ROOT,
    "src",
    "main",
    "game-delivery",
    "schema-validator.js"
  ));
  const schema = require(path.join(
    REPO_ROOT,
    "src",
    "main",
    "game-delivery",
    "schemas",
    "channel-manifest.schema.json"
  ));

  const validate = compileSchema(schema);

  if (!validate(data))
    for (const err of validate.errors || [])
      problems.push(`schema: ${err.instancePath || "/"} ${err.message}`);

  if (!semverParse(data.version)) problems.push(`version не SemVer: ${data.version}`);
  if (!semverParse(data.minimumLauncherVersion))
    problems.push(`minimumLauncherVersion не SemVer: ${data.minimumLauncherVersion}`);
  if (typeof data.updateRequired !== "boolean") problems.push("updateRequired должен быть boolean");

  if (typeof data.enabled !== "boolean")
    problems.push(`enabled должен быть boolean, получено ${JSON.stringify(data.enabled)}`);

  if (data.versionStatus !== undefined && typeof data.versionStatus !== "string")
    problems.push("versionStatus должен быть строкой");

  if (data.enabled === true && data.versionStatus !== "assigned")
    problems.push(`enabled=true требует versionStatus "assigned", получено ${JSON.stringify(data.versionStatus)}`);

  if (!opts.allowLocalHttp) {
    if (data.fullArchive) checkProviderUrls(data.fullArchive.providers, "fullArchive", problems);
    if (data.filesManifest) checkProviderUrls(data.filesManifest.providers, "filesManifest", problems);
    for (const repair of data.repairArchives || [])
      checkProviderUrls(repair.archive.providers, `repairArchives[${repair.version}]`, problems);
    if (data.repairMirror) checkProviderUrls(data.repairMirror.providers, "repairMirror", problems);
    for (const patch of data.patches || [])
      checkProviderUrls(patch.archive.providers, `patch ${patch.fromVersion}->${patch.toVersion}`, problems);
  }

  if (data.filesManifest && data.filesManifest.version !== data.version)
    problems.push(
      `filesManifest.version (${data.filesManifest.version}) не совпадает с version (${data.version})`
    );

  for (const patch of data.patches || []) {
    if (semverCompare(patch.fromVersion, patch.toVersion) >= 0)
      problems.push(`патч ${patch.fromVersion}->${patch.toVersion}: fromVersion не младше toVersion`);
    if (semverCompare(patch.toVersion, data.version) > 0)
      problems.push(`патч ${patch.fromVersion}->${patch.toVersion}: toVersion новее версии канала`);
  }

  return problems;
}

module.exports = {
  REPO_ROOT,
  PRIVATE_KEY_ENV,
  ALLOWED_HTTPS_HOSTS,
  sha256File,
  sha512File,
  keyIdOf,
  forbiddenKeyZones,
  isInside,
  assertPrivateKeyPathAllowed,
  loadPrivateKey,
  readJson,
  readStructured,
  BINARY_EXTENSIONS,
  writeJson,
  formatBytes,
  formatBytesShort,
  semverParse,
  semverCompare,
  validateChannelManifest,
};
