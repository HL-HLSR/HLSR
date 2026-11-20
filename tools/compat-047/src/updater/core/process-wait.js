"use strict";

const { spawnSync } = require("child_process");

const { CODES, UpdaterError } = require("./errors");

const DEFAULT_TIMEOUT_MS = 60000;
const POLL_INTERVAL_MS = 500;

function listPids(imageName, options) {
  const opts = options || {};
  const runner = opts.spawnSync || spawnSync;

  const result = runner(
    "tasklist",
    ["/FI", `IMAGENAME eq ${imageName}`, "/NH", "/FO", "CSV"],
    { windowsHide: true, encoding: "utf8" }
  );

  if (result.error || typeof result.stdout !== "string") return [];

  const pids = [];
  for (const line of result.stdout.split(/\r?\n/)) {
    const match = /^"([^"]+)","(\d+)"/.exec(line.trim());
    if (!match) continue;
    if (match[1].toLowerCase() !== imageName.toLowerCase()) continue;
    pids.push(Number(match[2]));
  }

  return pids;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitForExit(options) {
  const opts = options || {};
  const imageName = opts.imageName || "HLSR.exe";
  const timeoutMs = opts.timeoutMs == null ? DEFAULT_TIMEOUT_MS : opts.timeoutMs;
  const pollMs = opts.pollMs == null ? POLL_INTERVAL_MS : opts.pollMs;
  const list = opts.listPids || listPids;
  const onWait = opts.onWait || (() => {});
  const nap = opts.sleep || sleep;
  const now = opts.now || (() => Date.now());

  const deadline = now() + timeoutMs;
  let lastPids = [];

  for (;;) {
    lastPids = list(imageName, opts);
    if (!lastPids.length) return { ok: true, waitedFor: [] };

    if (now() >= deadline)
      throw new UpdaterError(
        CODES.HLSR_STILL_RUNNING,
        `HLSR не завершился за ${Math.round(timeoutMs / 1000)} с (PID: ${lastPids.join(", ")})`,
        { pids: lastPids }
      );

    onWait({ pids: lastPids, remainingMs: Math.max(0, deadline - now()) });
    await nap(pollMs);
  }
}

module.exports = { waitForExit, listPids, DEFAULT_TIMEOUT_MS };
