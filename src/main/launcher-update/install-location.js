"use strict";

const { execFileSync } = require("child_process");
const path = require("path");

const UNINSTALL_ROOT = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

const INSTALL_ROOT = "HKEY_CURRENT_USER\\Software";

const GUID_RE = /^\{?[0-9A-Za-z][0-9A-Za-z.-]{2,78}\}?$/;

function installKeyFrom(uninstallKey) {
  const guid = String(uninstallKey || "").split("\\").pop();

  return GUID_RE.test(guid) ? `${INSTALL_ROOT}\\${guid}` : null;
}

function runReg(args, exec) {
  const run = exec || execFileSync;

  try {
    return String(
      run("reg.exe", args, { encoding: "utf8", windowsHide: true, timeout: 15000 }) || ""
    );
  } catch (error) {
    return "";
  }
}

function parseUninstallEntries(output) {
  const lines = String(output || "").split(/\r?\n/);
  const entries = [];
  let currentKey = null;

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;

    if (/^HKEY_/i.test(trimmed)) {
      currentKey = trimmed;
      continue;
    }

    const match = trimmed.match(/^UninstallString\s+REG_[A-Z_]+\s+(.*)$/i);
    if (match && currentKey) entries.push({ key: currentKey, uninstallString: match[1].trim() });
  }

  return entries;
}

function executableFromUninstallString(value) {
  const raw = String(value || "").trim();
  if (!raw) return null;

  if (raw[0] === '"') {
    const end = raw.indexOf('"', 1);
    return end > 1 ? raw.slice(1, end) : null;
  }

  const match = raw.match(/^(.*?\.exe)(\s|$)/i);
  return match ? match[1] : null;
}

function insideDir(file, dir) {
  if (!file || !dir) return false;

  const parent = path.resolve(path.dirname(file)).toLowerCase();
  return parent === path.resolve(dir).toLowerCase();
}

function findInstallKey(installDir, options) {
  const opts = options || {};
  const output = runReg([
    "query",
    opts.root || UNINSTALL_ROOT,
    "/s",
    "/v",
    "UninstallString",
  ], opts.exec);

  for (const entry of parseUninstallEntries(output)) {
    if (insideDir(executableFromUninstallString(entry.uninstallString), installDir))
      return entry.key;
  }

  return null;
}

function readInstallLocation(key, options) {
  const opts = options || {};
  const output = runReg(["query", key, "/v", "InstallLocation"], opts.exec);
  const match = output.match(/^\s*InstallLocation\s+REG_[A-Z_]+\s+(.*)$/im);

  return match ? match[1].trim() : "";
}

function ensureInstallLocation(options) {
  const opts = options || {};
  const logger = opts.logger || { info() {}, warn() {} };
  const installDir = opts.installDir;

  if (process.platform !== "win32" && !opts.exec) return { status: "skipped" };
  if (!installDir) return { status: "no-key" };

  const uninstallKey = opts.uninstallKey || findInstallKey(installDir, opts);
  const key = opts.key || installKeyFrom(uninstallKey);

  if (!key) {
    logger.info("[update] своей записи установки в реестре нет — каталог не поправить");
    return { status: "no-key" };
  }

  const current = readInstallLocation(key, opts);

  if (current && path.resolve(current).toLowerCase() === path.resolve(installDir).toLowerCase())
    return { status: "ok", key, value: current };

  const output = runReg(
    ["add", key, "/v", "InstallLocation", "/t", "REG_SZ", "/d", installDir, "/f"],
    opts.exec
  );

  if (!output) {
    logger.warn(`[update] не удалось записать InstallLocation=${installDir}`);
    return { status: "write-failed", key, value: current };
  }

  logger.info(
    `[update] InstallLocation приведён к каталогу лаунчера: ` +
      `${current ? `было "${current}", ` : "значения не было, "}стало "${installDir}"`
  );

  return { status: "written", key, value: installDir };
}

module.exports = {
  ensureInstallLocation,
  findInstallKey,
  installKeyFrom,
  parseUninstallEntries,
  executableFromUninstallString,
  readInstallLocation,
  UNINSTALL_ROOT,
  INSTALL_ROOT,
};
