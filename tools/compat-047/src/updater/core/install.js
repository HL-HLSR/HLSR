"use strict";

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

const { CODES, UpdaterError } = require("./errors");

const INSTALLER_ARGS = ["/S", "--updated"];

const DEFAULT_INSTALL_TIMEOUT_MS = 10 * 60 * 1000;

const CHARS_NODE_SPAWN_QUOTES = /[\s"]/;

function nsisRejectsDSwitchQuotedBySpawn(dir) {
  return CHARS_NODE_SPAWN_QUOTES.test(dir);
}

function installerArgsFor(installDir) {
  const dir = typeof installDir === "string" ? installDir.trim() : "";
  if (!dir || nsisRejectsDSwitchQuotedBySpawn(dir)) return INSTALLER_ARGS.slice();

  const trailingUnquotedInstallDirArg = `/D=${dir}`;
  return INSTALLER_ARGS.concat([trailingUnquotedInstallDirArg]);
}

function runInstaller(installerPath, options) {
  const opts = options || {};
  const spawnFn = opts.spawn || spawn;
  const timeoutMs = opts.timeoutMs == null ? DEFAULT_INSTALL_TIMEOUT_MS : opts.timeoutMs;
  const args = opts.args || installerArgsFor(opts.installDir);

  return new Promise((resolve, reject) => {
    let child;

    try {
      child = spawnFn(installerPath, args, {
        windowsHide: !!opts.windowsHide,
        detached: false,
        stdio: "ignore",
        env: opts.env || process.env,
      });
    } catch (error) {
      return reject(new UpdaterError(CODES.INSTALL_FAILED, error ? error.message : "spawn"));
    }

    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;

      try {
        if (child && typeof child.kill === "function") child.kill();
      } catch (e) {
        /* процесс мог завершиться сам между таймером и этой строкой */
      }

      reject(new UpdaterError(CODES.INSTALL_FAILED, `установщик не завершился за ${timeoutMs} мс`));
    }, timeoutMs);

    child.on("error", (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(new UpdaterError(CODES.INSTALL_FAILED, error ? error.message : "spawn"));
    });

    child.on("exit", (code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);

      if (code !== 0)
        return reject(
          new UpdaterError(CODES.INSTALL_EXIT_CODE, `установщик вернул код ${code}`, {
            exitCode: code,
          })
        );

      resolve({ ok: true, exitCode: code });
    });
  });
}

// Сборка, собранная из чанков, ставится её же HLSR.exe в режиме тихой
// установки: так же, как при запуске HLSR-Setup.exe, только без распаковки.
// PORTABLE_EXECUTABLE_FILE включает режим установщика (windows-install/setup).
function treeSetupOptions(treeDir, installDir, version) {
  return {
    args: ["/S", "--updated", `/D=${installDir}`],
    env: Object.assign({}, process.env, { PORTABLE_EXECUTABLE_FILE: path.join(treeDir, `HLSR-Setup-${version}.exe`) }),
    windowsHide: true,
  };
}

function verifyInstalled(options) {
  const opts = options || {};
  const exePath = opts.launcherExe;
  const expected = opts.targetVersion;
  const readVersion = opts.readVersion || readExeProductVersion;

  if (!fs.existsSync(exePath))
    return { ok: false, code: CODES.INSTALL_NOT_APPLIED, detail: "HLSR.exe отсутствует" };

  const actual = readVersion(exePath);

  if (!actual) return { ok: true, version: null, unverified: true };

  const actualTriple = versionTriple(actual);
  const expectedTriple = versionTriple(expected);

  if (!actualTriple) return { ok: true, version: actual, unverified: true };

  if (expectedTriple && actualTriple !== expectedTriple)
    return {
      ok: false,
      code: CODES.INSTALL_NOT_APPLIED,
      detail: `на диске ${actual}, ожидалось ${expected}`,
      version: actual,
    };

  return { ok: true, version: actual };
}

function versionTriple(value) {
  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(String(value == null ? "" : value).trim());
  return match ? `${match[1]}.${match[2]}.${match[3]}` : null;
}

function readExeProductVersion(exePath) {
  const { spawnSync } = require("child_process");

  // По полному пути: каталог PowerShell бывает выпадает из PATH.
  const systemRoot = process.env.SystemRoot || process.env.WINDIR || "C:\\Windows";
  const result = spawnSync(
    path.win32.join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      `(Get-Item -LiteralPath '${exePath.replace(/'/g, "''")}').VersionInfo.ProductVersion`,
    ],
    { windowsHide: true, encoding: "utf8", timeout: 15000 }
  );

  if (result.error || typeof result.stdout !== "string") return null;

  const value = result.stdout.trim();
  return /^\d+\.\d+\.\d+/.test(value) ? value.split(/\s+/)[0] : null;
}

function relaunch(launcherExe, options) {
  const opts = options || {};
  const spawnFn = opts.spawn || spawn;

  if (!fs.existsSync(launcherExe))
    throw new UpdaterError(CODES.RELAUNCH_FAILED, "HLSR.exe не найден после установки");

  try {
    const child = spawnFn(launcherExe, [], {
      cwd: path.dirname(launcherExe),
      detached: true,
      stdio: "ignore",
      windowsHide: false,
    });

    if (child && typeof child.unref === "function") child.unref();
    return { ok: true, pid: child ? child.pid : null };
  } catch (error) {
    throw new UpdaterError(CODES.RELAUNCH_FAILED, error ? error.message : "spawn");
  }
}

module.exports = {
  runInstaller,
  installerArgsFor,
  treeSetupOptions,
  verifyInstalled,
  relaunch,
  readExeProductVersion,
  versionTriple,
  INSTALLER_ARGS,
  DEFAULT_INSTALL_TIMEOUT_MS,
};
