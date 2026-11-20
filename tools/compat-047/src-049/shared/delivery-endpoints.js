"use strict";

const PRIMARY_HOST = "hlsr.pro";
const FALLBACK_HOST = "github.com";
const FALLBACK_ASSET_HOST = "objects.githubusercontent.com";
// Half-Life 2005 WON пока качается с сайта автора BXT; архив закреплён по
// SHA-256 во встроенном манифесте. Подменённый или новый архив на сервере
// не ставится ни при установке, ни как обновление (origin-updates.js).
const BXT_HOST = "bxt.rs";

const PRIMARY_BASE = "https://hlsr.pro/downloads";

const LAUNCHER_RELEASES_REPO = "HL-HLSR/HLSR-Releases";
const GAME_RELEASES_REPO = "HL-HLSR/Ghosting-Renewed-Releases";

const RELEASES_REPO = LAUNCHER_RELEASES_REPO;

const GITHUB_RELEASE_BASE = `https://github.com/${LAUNCHER_RELEASES_REPO}/releases`;
const GAME_GITHUB_RELEASE_BASE = `https://github.com/${GAME_RELEASES_REPO}/releases`;

const GITHUB_ASSET_MAX_BYTES = 2 * 1024 * 1024 * 1024;

function fitsGithubAsset(size) {
  return Number.isFinite(size) && size > 0 && size < GITHUB_ASSET_MAX_BYTES;
}

// Windows и Linux публикуются одинаково: подписанный launcher-channel.json,
// установщик (exe или AppImage) и latest.yml / latest-linux.yml. Каталоги
// и теги разные (v* и linux-v*), поэтому каналы не пересекаются.
const LAUNCHER_CHUNKS_BASE = `${PRIMARY_BASE}/launcher/chunks`;

const LAUNCHER_BASES = {
  win32: {
    primary: `${PRIMARY_BASE}/launcher/stable`,
    fallback: `${GITHUB_RELEASE_BASE}/latest/download`,
    channelFile: "launcher-channel.json",
    latestYml: "latest.yml",
    tagPrefix: "v",
  },
  linux: {
    primary: `${PRIMARY_BASE}/launcher/linux/stable`,
    fallback: `${GITHUB_RELEASE_BASE}/download/linux-stable`,
    channelFile: "launcher-channel.json",
    latestYml: "latest-linux.yml",
    tagPrefix: "linux-v",
  },
};

function launcherFor(platform) {
  const base = LAUNCHER_BASES[platform === "linux" ? "linux" : "win32"];

  return {
    platform: platform === "linux" ? "linux" : "win32",
    primaryBase: base.primary,
    fallbackBase: base.fallback,
    channelUrls: [`${base.primary}/${base.channelFile}`, `${base.fallback}/${base.channelFile}`],
    latestYmlUrls: [`${base.primary}/${base.latestYml}`, `${base.fallback}/${base.latestYml}`],
    feedUrls: [base.primary, base.fallback],

    installerUrls(version, installerName) {
      return [
        `${base.primary}/${installerName}`,
        `${GITHUB_RELEASE_BASE}/download/${base.tagPrefix}${version}/${installerName}`,
      ];
    },

    releaseTag(version) {
      return `${base.tagPrefix}${version}`;
    },

    // Обновление чанками: манифест версии лежит рядом с установщиком, куски —
    // в общем для платформ хранилище. Только на hlsr.pro: в GitHub-релизе
    // нельзя держать тысячи файлов, оттуда клиент берёт полный установщик.
    chunkManifestUrls(filename) {
      return [`${base.primary}/${filename}`];
    },
    chunkBaseUrls() {
      return [LAUNCHER_CHUNKS_BASE];
    },
  };
}

const launcher = launcherFor(process.platform);

const LAUNCHER_PRIMARY_BASE = launcher.primaryBase;
const LAUNCHER_FALLBACK_BASE = launcher.fallbackBase;

const gamePrimaryBase = (gameId) => `${PRIMARY_BASE}/${gameId}`;

const game = {
  channelUrl(gameId) {
    return `${gamePrimaryBase(gameId)}/channel/stable.json`;
  },

  releaseTag(gameId, version) {
    return `game-${gameId}-v${version}`;
  },

  primaryFileUrl(gameId, version, filename) {
    return `${gamePrimaryBase(gameId)}/${version}/${filename}`;
  },

  fallbackFileUrl(gameId, version, filename) {
    return `${GAME_GITHUB_RELEASE_BASE}/download/${game.releaseTag(gameId, version)}/${filename}`;
  },

  releaseRepository() {
    return GAME_RELEASES_REPO;
  },

  providersFor(gameId, version, filename, size) {
    const providers = [
      {
        id: "hlsr-primary",
        type: "https-direct",
        status: "permanent",
        priority: 10,
        url: game.primaryFileUrl(gameId, version, filename),
      },
    ];

    if (size === undefined || fitsGithubAsset(size))
      providers.push({
        id: "github-fallback",
        type: "https-direct",
        status: "permanent",
        priority: 20,
        url: game.fallbackFileUrl(gameId, version, filename),
      });

    return providers;
  },
};

const ALLOWED_DOWNLOAD_HOSTS = [PRIMARY_HOST, FALLBACK_HOST, FALLBACK_ASSET_HOST, BXT_HOST];

const FORBIDDEN_DOWNLOAD_HOSTS = ["api.hlsr.pro", "disk.yandex.ru", "downloader.disk.yandex.ru", "storage.yandexcloud.net"];

function isAllowedDownloadUrl(value) {
  let parsed;
  try {
    parsed = new URL(String(value));
  } catch (e) {
    return false;
  }

  if (parsed.protocol !== "https:") return false;
  if (FORBIDDEN_DOWNLOAD_HOSTS.indexOf(parsed.hostname) !== -1) return false;
  return ALLOWED_DOWNLOAD_HOSTS.indexOf(parsed.hostname) !== -1;
}

module.exports = {
  PRIMARY_HOST,
  FALLBACK_HOST,
  FALLBACK_ASSET_HOST,
  BXT_HOST,
  PRIMARY_BASE,
  RELEASES_REPO,
  LAUNCHER_RELEASES_REPO,
  GAME_RELEASES_REPO,
  GITHUB_RELEASE_BASE,
  GAME_GITHUB_RELEASE_BASE,
  GITHUB_ASSET_MAX_BYTES,
  fitsGithubAsset,
  LAUNCHER_PRIMARY_BASE,
  LAUNCHER_FALLBACK_BASE,
  LAUNCHER_CHUNKS_BASE,
  ALLOWED_DOWNLOAD_HOSTS,
  FORBIDDEN_DOWNLOAD_HOSTS,
  isAllowedDownloadUrl,
  launcherFor,
  launcher,
  game,
};
