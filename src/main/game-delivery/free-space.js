"use strict";

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const CACHE_MS = 5000;

const cache = new Map();

function viaStatfs(dir) {
  if (typeof fs.statfsSync !== "function") return null;

  try {
    const stat = fs.statfsSync(dir);
    const bytes = stat.bavail * stat.bsize;

    return Number.isFinite(bytes) && bytes >= 0 ? bytes : null;
  } catch (error) {
    return null;
  }
}

function viaFsutil(dir, run) {
  if (process.platform !== "win32") return null;

  let root;
  try {
    root = path.parse(path.resolve(dir || "C:\\")).root;
  } catch (error) {
    return null;
  }

  if (!root) return null;

  let output;
  try {
    output = run("fsutil", ["volume", "diskfree", root], {
      timeout: 5000,
      windowsHide: true,
      encoding: "utf8",
    });
  } catch (error) {
    return null;
  }

  const first = String(output || "").split(/\r?\n/)[0] || "";
  const match = first.match(/:\s*([\d\s .,\u00a0]+?)(\s*\(|$)/);
  if (!match) return null;

  const bytes = Number(match[1].replace(/[\s .,\u00a0]/g, ""));

  return Number.isFinite(bytes) && bytes >= 0 ? bytes : null;
}

function freeSpaceBytes(dir, options) {
  const opts = options || {};
  const now = typeof opts.now === "function" ? opts.now() : Date.now();
  const run = opts.run || execFileSync;

  const key = String(dir || "");
  const hit = cache.get(key);
  if (hit && now - hit.at < CACHE_MS) return hit.value;

  let value = viaStatfs(dir);
  if (value === null) value = viaFsutil(dir, run);

  cache.set(key, { at: now, value });

  return value;
}

function forget() {
  cache.clear();
}

module.exports = { freeSpaceBytes, forget, viaStatfs, viaFsutil, CACHE_MS };
