"use strict";

const fs = require("fs");
const path = require("path");

const { CODES, UpdaterError } = require("./errors");

const LOCK_NAME = "updater.lock";

function isProcessAlive(pid, options) {
  const opts = options || {};
  if (opts.isProcessAlive) return opts.isProcessAlive(pid);

  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error && error.code === "EPERM";
  }
}

function acquire(dir, options) {
  const opts = options || {};
  const pid = opts.pid == null ? process.pid : opts.pid;
  const file = path.join(dir, LOCK_NAME);

  fs.mkdirSync(dir, { recursive: true });

  const takeOver = () => {
    fs.writeFileSync(file, JSON.stringify({ pid, ts: Date.now() }), "utf8");
    return {
      file,
      release() {
        try {
          const raw = JSON.parse(fs.readFileSync(file, "utf8"));
          if (raw && raw.pid === pid) fs.unlinkSync(file);
        } catch (e) {
          /* уже снят */
        }
      },
    };
  };

  let existing = null;
  try {
    existing = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    return takeOver();
  }

  const otherPid = existing && Number(existing.pid);

  if (!otherPid || otherPid === pid) return takeOver();

  if (isProcessAlive(otherPid, opts))
    throw new UpdaterError(
      CODES.ALREADY_RUNNING,
      `обновление уже выполняется (PID ${otherPid})`,
      { pid: otherPid }
    );

  return takeOver();
}

module.exports = { acquire, isProcessAlive, LOCK_NAME };
