"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { spawn } = require("child_process");
const fsCompat = require("./fs-compat");
const updaterMode = require("./updater-mode");

const SKIP_ENTRIES = new Set(["resources", "LICENSES.chromium.html"]);

const UPDATER_EXE = "HLSRUpdater.exe";

const QA_TRUST = "QA_TRUST";

const STARTUP_TIMEOUT_MS = 15000;

function uniqueDir(tempRoot) {
  const suffix = crypto.randomBytes(6).toString("hex");
  return path.join(tempRoot || os.tmpdir(), `HLSR-Updater-${Date.now().toString(36)}-${suffix}`);
}

function stage(options) {
  const opts = options || {};
  const appDir = opts.appDir;
  const resourcesPath = opts.resourcesPath;
  const dir = opts.dir || uniqueDir(opts.tempRoot);

  // Апдейтер лежит в app.asar лаунчера (updater-mode.js): копия exe
  // запускается с копией того же app.asar, другой asar exe не примет.
  // original-fs: обычный fs в Electron видит app.asar каталогом.
  const asarSource = path.join(resourcesPath, "app.asar");

  if (!fsCompat.existsSync(asarSource))
    throw new Error(`не найден app.asar в ресурсах: ${asarSource}`);

  fs.mkdirSync(dir, { recursive: true });

  for (const entry of fs.readdirSync(appDir)) {
    if (SKIP_ENTRIES.has(entry)) continue;

    const from = path.join(appDir, entry);
    const to = path.join(dir, entry === "HLSR.exe" ? UPDATER_EXE : entry);

    fs.cpSync(from, to, { recursive: true, force: true });
  }

  const resourcesTarget = path.join(dir, "resources");
  fs.mkdirSync(resourcesTarget, { recursive: true });
  fsCompat.copyFileSync(asarSource, path.join(resourcesTarget, "app.asar"));

  // Ключи QA-канала для апдейтера (src/updater/main.js, qaTrustOptions).
  const qaTrust = path.join(resourcesPath, QA_TRUST);
  if (fs.existsSync(qaTrust)) fs.copyFileSync(qaTrust, path.join(resourcesTarget, QA_TRUST));

  const exe = path.join(dir, UPDATER_EXE);
  if (!fs.existsSync(exe)) throw new Error("копия Updater не собралась");

  return { dir, exe, planFile: path.join(dir, "update-plan.json") };
}

function writePlan(planFile, plan) {
  fs.writeFileSync(planFile, JSON.stringify(plan, null, 2), "utf8");
  return planFile;
}

function spawnUpdater(exe, planFile, options) {
  const opts = options || {};
  const spawnFn = opts.spawn || spawn;

  const child = spawnFn(exe, [updaterMode.FLAG, `--plan=${planFile}`], {
    cwd: path.dirname(exe),
    detached: true,
    stdio: "ignore",
    windowsHide: false,
  });

  if (!child || !child.pid) throw new Error("процесс Updater не запустился");
  if (typeof child.unref === "function") child.unref();

  return { pid: child.pid, child };
}

function waitForStartup(pid, options) {
  const opts = options || {};
  const timeoutMs = opts.timeoutMs == null ? STARTUP_TIMEOUT_MS : opts.timeoutMs;
  const stableMs = opts.stableMs == null ? 1200 : opts.stableMs;
  const isAlive = opts.isProcessAlive || defaultIsAlive;
  const now = opts.now || (() => Date.now());
  const sleep = opts.sleep || ((ms) => new Promise((r) => setTimeout(r, ms)));

  return (async () => {
    const deadline = now() + timeoutMs;

    const until = now() + stableMs;

    while (now() < deadline) {
      if (!isAlive(pid)) return { ok: false, reason: "процесс завершился сразу после запуска" };

      if (now() >= until) return { ok: true };

      await sleep(Math.min(300, stableMs));
    }

    return { ok: false, reason: "таймаут ожидания запуска" };
  })();
}

function defaultIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error && error.code === "EPERM";
  }
}

async function launch(options) {
  const opts = options || {};
  const logger = opts.logger || { info() {}, warn() {}, error() {} };

  const staged = (opts.stage || stage)({
    appDir: opts.appDir,
    resourcesPath: opts.resourcesPath,
    tempRoot: opts.tempRoot,
  });

  logger.info(`[updater-launch] копия развёрнута: ${path.basename(staged.dir)}`);

  (opts.writePlan || writePlan)(staged.planFile, opts.plan);

  const { pid } = (opts.spawnUpdater || spawnUpdater)(staged.exe, staged.planFile, opts);
  logger.info(`[updater-launch] Updater запущен, PID ${pid}`);

  const startup = await (opts.waitForStartup || waitForStartup)(pid, opts);

  if (!startup.ok) {
    logger.error(`[updater-launch] Updater не поднялся: ${startup.reason}`);
    throw new Error(startup.reason);
  }

  return { ok: true, pid, dir: staged.dir, exe: staged.exe, planFile: staged.planFile };
}

function cleanupOldStages(options) {
  const opts = options || {};
  const tempRoot = opts.tempRoot || os.tmpdir();
  const maxAgeMs = opts.maxAgeMs == null ? 24 * 60 * 60 * 1000 : opts.maxAgeMs;
  const now = opts.now || (() => Date.now());

  let removed = 0;

  let entries;
  try {
    entries = fs.readdirSync(tempRoot);
  } catch (error) {
    return { removed: 0 };
  }

  for (const entry of entries) {
    if (!/^HLSR-Updater-[a-z0-9]+-[a-f0-9]{12}$/.test(entry)) continue;

    const full = path.join(tempRoot, entry);

    try {
      const stat = fs.statSync(full);
      if (!stat.isDirectory()) continue;
      if (now() - stat.mtimeMs < maxAgeMs) continue;

      fs.rmSync(full, { recursive: true, force: true });
      removed++;
    } catch (error) {
      /* занятый каталог удалится в следующий раз */
    }
  }

  return { removed };
}

module.exports = {
  stage,
  writePlan,
  spawnUpdater,
  waitForStartup,
  launch,
  cleanupOldStages,
  uniqueDir,
  UPDATER_EXE,
  SKIP_ENTRIES,
};
