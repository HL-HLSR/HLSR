"use strict";

const DEFAULT_TIMEOUT_MS = 10000;

const STATES = {
  IDLE: "idle",
  CHECKING: "checking",
  AVAILABLE: "available",
  NOT_AVAILABLE: "not-available",
  DOWNLOADING: "downloading",
  VERIFYING: "verifying",
  DOWNLOADED: "downloaded",
  ERROR: "error",
};

function runBootGate(host) {
  const log = host.log || { info() {}, warn() {} };
  const timeoutMs = host.timeoutMs == null ? DEFAULT_TIMEOUT_MS : host.timeoutMs;

  if (!host.enabled)
    return Promise.resolve({ action: "proceed", reason: "updates-disabled" });

  return new Promise((resolve) => {
    let settled = false;
    let checkTimer = null;
    let unsubscribe = null;

    const settle = (decision) => {
      if (settled) return;
      settled = true;
      if (checkTimer) clearTimeout(checkTimer);
      if (unsubscribe) unsubscribe();
      resolve(decision);
    };

    checkTimer = setTimeout(() => {
      const state = host.getState();
      if (state === STATES.CHECKING || state === STATES.IDLE) {
        log.warn("[boot] update check timed out — starting current version");
        settle({ action: "proceed", reason: "check-timeout", warn: "UPDATE_CHECK_TIMEOUT" });
      }
    }, timeoutMs);

    unsubscribe = host.subscribe((state) => {
      if (state === STATES.NOT_AVAILABLE)
        return settle({ action: "proceed", reason: "no-update" });

      if (state === STATES.AVAILABLE) {
        if (checkTimer) clearTimeout(checkTimer);
        log.info("[boot] update available — downloading on loading screen");
        host.download();
        return;
      }

      if (state === STATES.DOWNLOADED) {
        if (host.isGameRunning())
          return settle({ action: "proceed", reason: "game-running" });

        log.info("[boot] verified update ready — installing");
        settle({
          action: "installing",
          reason: "installing",
          version: host.getVersion(),
        });
        host.install();
        return;
      }

      if (state === STATES.ERROR) {
        const error = host.getError() || "unknown";

        if (host.isMandatory())
          return settle({
            action: "blocked",
            reason: "mandatory-update-failed",
            mandatory: true,
            error,
          });

        return settle({
          action: "proceed",
          reason: "update-error",
          warn: "LU_FEED_UNAVAILABLE",
          error,
        });
      }
    });

    host.check();
  });
}

module.exports = { runBootGate, STATES, DEFAULT_TIMEOUT_MS };
