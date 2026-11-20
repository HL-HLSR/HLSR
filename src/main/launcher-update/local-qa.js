"use strict";

// Как у Windows, локальный канал доступен только сборке с вшитым qa.
// В production webpack заменяет условие константой false: переменные
// окружения пользователя не меняют адреса и ключи доверия обновлений.
const LauncherUpdateGate = require("./LauncherUpdateGate");
const { download } = require("./ArchiveDownload");
const http = require("http");
const path = require("path");

const IS_QA_BUILD = process.env.HLSR_BUILD_CHANNEL === "qa";

function configuration(currentVersion, logger) {
  if (!IS_QA_BUILD || !process.env.HLSR_UPDATE_FEED) return null;
  let base;
  try { base = new URL(process.env.HLSR_UPDATE_FEED); } catch (_) { return null; }
  if (base.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(base.hostname)
      || base.username || base.password || base.search || base.hash) return null;
  const feed = base.href.replace(/\/+$/, "");
  const key = process.env.HLSR_UPDATE_CHANNEL_KEY;
  logger.info(`[updater] локальный QA-канал: ${feed}`);
  return {
    gate: new LauncherUpdateGate({ currentVersion, logger,
      channelUrls: [`${feed}/launcher-channel.json`], latestYmlUrls: [`${feed}/latest-linux.yml`],
      httpGet: LauncherUpdateGate.qaHttpGet,
      publicKeys: key ? [key.replace(/\\n/g, "\n")] : undefined,
    }),
    download: (manifest, directory, options) => download(manifest, directory, {
      ...options,
      // Подписанные URL и проверка allowlist остаются боевыми; только
      // транспорт QA-сборки берёт байты того же имени на loopback.
      transport: { get(url, requestOptions, callback) {
        const file = path.posix.basename(new URL(url).pathname);
        return http.get(`${feed}/${encodeURIComponent(file)}`, requestOptions, callback);
      } },
    }),
  };
}

module.exports = { configuration };
