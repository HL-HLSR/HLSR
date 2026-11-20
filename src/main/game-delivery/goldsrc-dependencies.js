"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { Transform } = require("stream");
const { pipeline } = require("stream/promises");

const httpClient = require("./http-client");
const { assertInside } = require("./paths");
const { DeliveryError, toDeliveryError } = require("./errors");

// Перед запуском HL1 лаунчер делает то же, что update.bat из сборки bxt.rs:
// берёт последний релиз с GitHub, сверяет его тег с записанной версией и при
// расхождении подменяет файл. Сбой обновления запуск не останавливает: игра
// стартует со старой версией, а интерфейс предупреждает, что ран с устаревшим
// BXT может не засчитаться.

const API_ROOT = "https://api.github.com/repos/";
const CHECK_TIMEOUT_MS = 10000;
const DOWNLOAD_TIMEOUT_MS = 90000;
const MAX_ASSET_BYTES = 32 * 1024 * 1024;

const REPO = /^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/;
const TAG = /^[0-9A-Za-z][0-9A-Za-z._-]{0,63}$/;
const SHA256 = /^sha256:([a-f0-9]{64})$/;

function insideInstall(installPath, relative) {
  return assertInside(installPath, path.join(installPath, String(relative).replace(/\//g, path.sep)), "Зависимость");
}

function readVersion(file, fs) {
  try {
    return fs.readFileSync(file, "utf8").replace(/^\uFEFF/, "").trim() || null;
  } catch (error) {
    return null;
  }
}

async function latestRelease(dependency, client) {
  if (!REPO.test(dependency.repo))
    throw new DeliveryError("MANIFEST_INVALID", `Недопустимый репозиторий ${dependency.repo}`);

  const pin = dependency.pin || null;
  const data = await client.getJson(`${API_ROOT}${dependency.repo}/releases/${pin ? `tags/${pin.tag}` : "latest"}`, {
    timeout: CHECK_TIMEOUT_MS,
    headers: { accept: "application/vnd.github+json" },
  });

  const tag = data && typeof data.tag_name === "string" ? data.tag_name : "";
  if (!TAG.test(tag) || (pin && tag !== pin.tag))
    throw new DeliveryError("PROVIDER_LINK_FAILED", `GitHub вернул недопустимый тег релиза: ${tag}`);

  const pattern = new RegExp(dependency.asset, "i");
  const asset = (Array.isArray(data.assets) ? data.assets : []).find((item) => item && pattern.test(item.name));
  if (!asset || typeof asset.browser_download_url !== "string")
    throw new DeliveryError("PROVIDER_LINK_FAILED", `В релизе ${tag} нет файла ${dependency.asset}`);

  const size = Number(asset.size);
  if (!Number.isInteger(size) || size <= 0 || size > MAX_ASSET_BYTES)
    throw new DeliveryError("PROVIDER_LINK_FAILED", `Недопустимый размер ${asset.name}: ${asset.size}`);

  const url = new URL(asset.browser_download_url);
  if (url.protocol !== "https:" || url.hostname !== "github.com")
    throw new DeliveryError("PROVIDER_LINK_FAILED", `Файл релиза лежит не на github.com: ${url.hostname}`);

  // Хеш закреплённой версии лежит в лаунчере. Иначе — SHA-256, который GitHub отдаёт для файлов
  // релиза; у старых релизов его может не быть, и тогда файл не ставится.
  const digest = SHA256.exec(String(asset.digest || ""));
  const sha256 = pin ? pin.sha256 : digest ? digest[1] : null;
  if (!sha256) throw new DeliveryError("CHECKSUM_MISMATCH", `GitHub не сообщил SHA-256 файла ${asset.name}`);

  return { tag, name: asset.name, url: url.href, size, sha256 };
}

async function download(release, file, client, fs) {
  const response = await client.request(release.url, { timeout: DOWNLOAD_TIMEOUT_MS });
  if (response.statusCode !== 200) {
    response.resume();
    throw new DeliveryError("HTTP_ERROR", `HTTP ${response.statusCode} для ${release.name}`);
  }

  const hash = crypto.createHash("sha256");
  let size = 0;
  const meter = new Transform({
    transform(chunk, encoding, callback) {
      size += chunk.length;
      if (size > release.size) {
        callback(new DeliveryError("SIZE_MISMATCH", `${release.name} больше объявленных ${release.size} Б`));
        return;
      }
      hash.update(chunk);
      callback(null, chunk);
    },
  });

  try {
    await pipeline(response, meter, fs.createWriteStream(file));
  } catch (error) {
    throw toDeliveryError(error, "NETWORK_ERROR");
  }

  if (size !== release.size)
    throw new DeliveryError("SIZE_MISMATCH", `Получено ${size} Б из ${release.size} для ${release.name}`);

  const digest = hash.digest("hex");
  if (digest !== release.sha256)
    throw new DeliveryError("CHECKSUM_MISMATCH", `SHA-256 ${release.name} не совпал`, {
      expected: release.sha256,
      actual: digest,
    });
}

function assertWindowsBinary(file, fs) {
  const head = Buffer.alloc(2);
  const fd = fs.openSync(file, "r");
  try {
    fs.readSync(fd, head, 0, 2, 0);
  } finally {
    fs.closeSync(fd);
  }
  if (head.toString("latin1") !== "MZ")
    throw new DeliveryError("EXTRACT_FAILED", `${path.basename(file)} не похож на исполняемый файл Windows`);
}

async function updateOne(installPath, dependency, options) {
  const { client, archive, fs, logger } = options;
  const target = insideInstall(installPath, dependency.target);
  const versionFile = insideInstall(installPath, dependency.versionFile);
  const current = readVersion(versionFile, fs);
  const result = { id: dependency.id, title: dependency.title, from: current, to: current, status: "up-to-date" };

  if (dependency.pin && dependency.pin.tag === current && fs.existsSync(target)) return result;

  const release = await latestRelease(dependency, client);
  if (release.tag === current && fs.existsSync(target)) return result;

  const dir = path.dirname(target);
  const temp = path.join(dir, `.hlsr-update-${dependency.id}-${process.pid}`);
  const downloadFile = `${temp}.download`;

  try {
    fs.rmSync(temp, { recursive: true, force: true });
    await download(release, downloadFile, client, fs);

    let fresh = downloadFile;
    if (dependency.extract) {
      if (!archive) throw new DeliveryError("EXTRACT_FAILED", "Нет 7-Zip для распаковки обновления");
      await archive.extract(downloadFile, temp).promise;
      fresh = insideInstall(temp, dependency.extract);
      if (!fs.existsSync(fresh))
        throw new DeliveryError("EXTRACT_FAILED", `В ${release.name} нет ${dependency.extract}`);
    }

    assertWindowsBinary(fresh, fs);
    fs.renameSync(fresh, target);
    fs.writeFileSync(versionFile, release.tag);
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
    fs.rmSync(downloadFile, { force: true });
  }

  logger.info(`[launch] ${dependency.title} обновлён: ${current || "неизвестная версия"} -> ${release.tag}`);
  return Object.assign(result, { to: release.tag, status: "updated" });
}

// Обновляет зависимости по очереди и никогда не бросает: результат каждой
// уходит в интерфейс, а запуск продолжается.
async function updateDependencies(installPath, config, options = {}) {
  const dependencies = (config && config.dependencies) || [];
  const opts = {
    client: options.client || httpClient,
    archive: options.archive || null,
    fs: options.fs || nodeFs,
    logger: options.logger || { info() {}, warn() {}, error() {} },
  };

  const results = [];
  for (const dependency of dependencies) {
    try {
      results.push(await updateOne(installPath, dependency, opts));
    } catch (error) {
      const delivery = toDeliveryError(error, "NETWORK_ERROR");
      opts.logger.warn(`[launch] ${dependency.title} не обновлён: ${delivery.code} ${delivery.message}`);
      results.push({
        id: dependency.id,
        title: dependency.title,
        from: readVersion(insideInstall(installPath, dependency.versionFile), opts.fs),
        to: null,
        status: "failed",
        error: { code: delivery.code, message: delivery.message },
      });
    }
  }
  return results;
}

module.exports = { updateDependencies, latestRelease, MAX_ASSET_BYTES };
