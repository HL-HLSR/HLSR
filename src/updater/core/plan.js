"use strict";

const path = require("path");

const channel = require("../shared/launcher-update/launcher-channel");
const { CODES, UpdaterError } = require("./errors");

function expectedInstallerName(version) {
  return `HLSR-Setup-${version}.exe`;
}

function isPlausibleLauncherDir(dir) {
  if (typeof dir !== "string" || !dir) return false;
  if (!path.isAbsolute(dir)) return false;
  if (dir.indexOf("..") !== -1) return false;
  if (/^\\\\/.test(dir)) return false;

  return /(^|[\\/])HLSR[^\\/]*([\\/]|$)/i.test(dir);
}

function validate(raw, options) {
  const opts = options || {};
  const publicKeys = opts.publicKeys || channel.LAUNCHER_PUBLIC_KEYS;

  if (!raw || typeof raw !== "object" || Array.isArray(raw))
    throw new UpdaterError(CODES.BAD_PLAN, "план не является объектом");

  const manifest = raw.manifest;
  const currentVersion = raw.currentVersion;
  const targetVersion = raw.targetVersion;

  if (!manifest || typeof manifest !== "object")
    throw new UpdaterError(CODES.BAD_PLAN, "в плане нет манифеста канала");

  const verdict = channel.check(manifest, { currentVersion, publicKeys });

  if (!verdict.ok)
    throw new UpdaterError(
      verdict.code === channel.CODES.SCHEMA && /подпись/.test(verdict.problems.join("; "))
        ? CODES.SIGNATURE_INVALID
        : verdict.code || CODES.SIGNATURE_INVALID,
      verdict.problems.join("; ")
    );

  if (!targetVersion || manifest.version !== targetVersion)
    throw new UpdaterError(
      CODES.VERSION_MISMATCH,
      `план обещает ${targetVersion}, подписано ${manifest.version}`
    );

  if (!verdict.updateAvailable)
    throw new UpdaterError(
      CODES.VERSION_MISMATCH,
      `${manifest.version} не новее установленной ${currentVersion}`
    );

  const wantName = expectedInstallerName(manifest.version);
  if (manifest.installer.filename !== wantName)
    throw new UpdaterError(
      CODES.FILENAME_MISMATCH,
      `installer.filename=${manifest.installer.filename}, ожидалось ${wantName}`
    );

  const urls = [];
  for (const url of manifest.installer.urls) {
    if (!channel.isAllowedUrl(url))
      throw new UpdaterError(CODES.URL_NOT_ALLOWED, `URL вне allowlist: ${url}`);

    let parsed;
    try {
      parsed = new URL(url);
    } catch (e) {
      throw new UpdaterError(CODES.URL_NOT_ALLOWED, `неразбираемый URL`);
    }

    const last = decodeURIComponent(parsed.pathname.split("/").pop() || "");
    if (last !== wantName)
      throw new UpdaterError(
        CODES.FILENAME_MISMATCH,
        `URL указывает на ${last}, ожидалось ${wantName}`
      );

    urls.push(url);
  }

  if (!urls.length) throw new UpdaterError(CODES.BAD_PLAN, "в манифесте нет URL установщика");

  const installDir = raw.installDir;
  if (!isPlausibleLauncherDir(installDir))
    throw new UpdaterError(CODES.BAD_PLAN, "installDir не похож на каталог HLSR");

  const launcherExe = raw.launcherExe;
  if (
    typeof launcherExe !== "string" ||
    !path.isAbsolute(launcherExe) ||
    !/\.exe$/i.test(launcherExe) ||
    path.dirname(launcherExe).toLowerCase() !== String(installDir).toLowerCase()
  )
    throw new UpdaterError(CODES.BAD_PLAN, "launcherExe вне каталога установки");

  const receiptFile = raw.receiptFile;
  if (
    typeof receiptFile !== "string" ||
    !path.isAbsolute(receiptFile) ||
    path.basename(receiptFile) !== "version-state.json"
  )
    throw new UpdaterError(CODES.BAD_PLAN, "receiptFile недопустим");

  const downloadUrls =
    typeof opts.downloadBase === "string" && opts.downloadBase
      ? [`${opts.downloadBase.replace(/\/+$/, "")}/${wantName}`]
      : urls;

  return {
    manifest,
    currentVersion,
    targetVersion: manifest.version,
    installer: {
      filename: wantName,
      size: manifest.installer.size,
      sha256: manifest.installer.sha256,
      sha512: manifest.installer.sha512,
      urls: downloadUrls,
    },
    installDir,
    launcherExe,
    receiptFile,
    language: raw.language === "en" ? "en" : "ru",
  };
}

module.exports = { validate, expectedInstallerName, isPlausibleLauncherDir };
