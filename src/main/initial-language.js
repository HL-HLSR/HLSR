"use strict";

const { execFileSync } = require("child_process");

const EN = 0;
const RU = 1;

const REGISTRY_KEY = "HKCU\\Software\\HLSR";
const REGISTRY_VALUE = "InstallerOrigin";

function installerOrigin(options) {
  const opts = options || {};
  if (process.platform !== "win32") return null;

  const run = opts.run || execFileSync;

  let output;
  try {
    output = run("reg.exe", ["query", REGISTRY_KEY, "/v", REGISTRY_VALUE], {
      timeout: 3000,
      windowsHide: true,
      encoding: "utf8",
    });
  } catch (error) {
    return null;
  }

  const match = String(output || "").match(/REG_SZ\s+(\S+)/);
  if (!match) return null;

  const value = match[1].trim().toLowerCase();

  return value === "ru" || value === "en" ? value : null;
}

function systemLanguage(locale) {
  const value = String(locale || "").toLowerCase();

  return value === "ru" || value.indexOf("ru-") === 0 ? "ru" : "en";
}

/**
 * Индекс языка для первого запуска.
 *
 * @param {object} options
 * @param {string} options.locale  app.getLocale()
 * @param {function} [options.run] подмена запуска reg.exe для тестов
 */
function pick(options) {
  const opts = options || {};

  const origin = installerOrigin(opts);
  if (origin) return origin === "ru" ? RU : EN;

  return systemLanguage(opts.locale) === "ru" ? RU : EN;
}

module.exports = { pick, installerOrigin, systemLanguage, EN, RU, REGISTRY_KEY, REGISTRY_VALUE };
