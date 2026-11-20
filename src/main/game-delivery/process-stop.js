"use strict";

const { execFile } = require("child_process");

function run(command, args, timeoutMs) {
  return new Promise((resolve) => {
    execFile(
      command,
      args,
      { windowsHide: true, timeout: timeoutMs || 10000 },
      (error, stdout, stderr) => {
        resolve({
          ok: !error,
          code: error ? error.code : 0,
          output: String(stdout || "") + String(stderr || ""),
        });
      }
    );
  });
}

function graceful(pid) {
  if (process.platform !== "win32") {
    try {
      process.kill(pid, "SIGTERM");
      return Promise.resolve({ ok: true, code: 0, output: "SIGTERM" });
    } catch (error) {
      return Promise.resolve({ ok: false, code: error.code, output: error.message });
    }
  }

  return run("taskkill.exe", ["/PID", String(pid), "/T"]);
}

function force(pid) {
  if (process.platform !== "win32") {
    try {
      process.kill(pid, "SIGKILL");
      return Promise.resolve({ ok: true, code: 0, output: "SIGKILL" });
    } catch (error) {
      return Promise.resolve({ ok: false, code: error.code, output: error.message });
    }
  }

  return run("taskkill.exe", ["/PID", String(pid), "/T", "/F"]);
}

// Proton запускает игру в своей группе процессов: на Linux её закрывают
// сигналом всей группе. В Windows taskkill /T и так закрывает дерево.
function signalGroup(pid, type) {
  if (process.platform === "win32") return type === "SIGKILL" ? force(pid) : graceful(pid);

  try {
    process.kill(-pid, type);
    return Promise.resolve({ ok: true, code: 0, output: type });
  } catch (error) {
    return Promise.resolve({ ok: false, code: error.code, output: error.message });
  }
}

module.exports = {
  graceful,
  force,
  gracefulGroup: (pid) => signalGroup(pid, "SIGTERM"),
  forceGroup: (pid) => signalGroup(pid, "SIGKILL"),
};
