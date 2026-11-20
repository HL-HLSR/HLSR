"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");

const redact = require("./redact");
const logPolicy = require("./log-policy");

const MAX_LOG_BYTES = 1024 * 1024;

function timestamp(date) {
  const d = date || new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return (
    `${d.getUTCFullYear()}${pad(d.getUTCMonth() + 1)}${pad(d.getUTCDate())}` +
    `-${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}${pad(d.getUTCSeconds())}`
  );
}

function tailBytes(file, limit) {
  const size = fs.statSync(file).size;
  const take = Math.min(size, limit);
  const buffer = Buffer.alloc(take);
  const fd = fs.openSync(file, "r");

  try {
    fs.readSync(fd, buffer, 0, take, size - take);
  } finally {
    fs.closeSync(fd);
  }

  return buffer.toString("utf8");
}

function buildManifest(input) {
  const info = input || {};

  return {
    schema: 1,
    generatedAt: info.now || "1970-01-01T00:00:00.000Z",
    app: {
      name: info.appName || "",
      version: info.appVersion || "",
      buildChannel: info.buildChannel || "production",
    },
    runtime: {
      electron: info.electronVersion || "",
      chrome: info.chromeVersion || "",
      node: info.nodeVersion || "",
      arch: info.arch || "",
    },
    os: {
      platform: info.platform || "",
      release: info.osRelease || "",
    },
    library: {
      path: redact.redactText(info.libraryPath || "", info),
      exists: Boolean(info.libraryExists),
    },
    update: {
      state: info.updateState || "idle",
      lastErrorCode: info.updateErrorCode || null,
    },
    env: redact.safeEnv(info.env || {}, info),
  };
}

function collect(options) {
  const opts = options || {};
  const outDir = opts.outDir;
  if (!outDir) throw new Error("не задан каталог отчёта");

  fs.mkdirSync(outDir, { recursive: true });

  const manifest = buildManifest(opts);
  const files = [];

  const manifestFile = path.join(outDir, "manifest.json");
  fs.writeFileSync(manifestFile, JSON.stringify(manifest, null, 2) + "\n", "utf8");
  files.push(manifestFile);

  if (opts.logsDir && fs.existsSync(opts.logsDir)) {
    const logs = logPolicy.logFiles(opts.logsDir, opts.logName || "main.log", opts)
      .concat(logPolicy.gameLogFiles(opts.logsDir));

    for (const file of logs) {
      const resolved = path.resolve(file);
      const root = path.resolve(opts.logsDir);
      if (resolved !== root && !resolved.toLowerCase().startsWith(root.toLowerCase() + path.sep))
        continue;

      let text;
      try {
        text = tailBytes(resolved, opts.maxLogBytes || MAX_LOG_BYTES);
      } catch (error) {
        continue;
      }

      const target = path.join(outDir, path.basename(resolved));
      fs.writeFileSync(target, redact.redactText(text, opts), "utf8");
      files.push(target);
    }
  }

  return { dir: outDir, files, manifest };
}

function pack(dir, archivePath) {
  const binary = require("../seven-zip").binPath();

  const { spawnSync } = require("child_process");
  const result = spawnSync(binary, ["a", "-tzip", "-y", archivePath, path.join(dir, "*")], {
    windowsHide: true,
    timeout: 60000,
  });

  if (result.error || result.status !== 0) return null;
  return fs.existsSync(archivePath) ? archivePath : null;
}

function exportDiagnostics(options) {
  const opts = options || {};
  const baseDir = opts.targetDir || path.join(os.tmpdir(), "hlsr-diagnostics");
  const stamp = timestamp(opts.date);
  const dir = path.join(baseDir, `hlsr-diagnostics-${stamp}`);

  try {
    const collected = collect(Object.assign({}, opts, { outDir: dir }));
    const archive = opts.pack === false ? null : pack(dir, `${dir}.zip`);

    return { ok: true, path: archive || dir, packed: Boolean(archive), files: collected.files };
  } catch (error) {
    return { ok: false, error: String((error && error.message) || error) };
  }
}

module.exports = { MAX_LOG_BYTES, timestamp, buildManifest, collect, pack, exportDiagnostics };
