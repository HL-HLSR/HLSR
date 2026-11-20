"use strict";

const { redactText } = require("../diagnostics/redact");

const OUTPUT_LIMIT = 8 * 1024;
const DEFAULT_TIMEOUT_MS = 5 * 60 * 1000;
const DEFAULT_KILL_GRACE_MS = 2000;

function cleanOutput(value) {
  // UMU_LOG=debug may print inherited environment variables. Keep diagnostics
  // useful without copying credentials from that environment into the log.
  return redactText(String(value)
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\b[A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|API_KEY|COOKIE|AUTH)[A-Z0-9_]*\s*=\s*[^\r\n]*/gi, "<redacted environment>"))
    .slice(-OUTPUT_LIMIT).trim();
}

function terminate(child, signal) {
  // Prefix commands are spawned in their own process group. Stop the bootstrap
  // and its new helpers together, without touching other prefixes or games.
  if (Number.isSafeInteger(child.pid) && child.pid > 0) {
    try { process.kill(-child.pid, signal); return; } catch (error) { /* fallback */ }
  }
  try { child.kill(signal); } catch (error) { /* already stopped */ }
}

function waitForPrefixCommand(child, options = {}) {
  const label = options.label || "umu-run";
  const timeoutMs = options.timeoutMs == null ? DEFAULT_TIMEOUT_MS : options.timeoutMs;
  const killGraceMs = options.killGraceMs == null ? DEFAULT_KILL_GRACE_MS : options.killGraceMs;
  return new Promise((resolve, reject) => {
    let stdout = "", stderr = "", timer = null, killTimer = null, settled = false, timedOut = false;
    const onStdout = (chunk) => { stdout = (stdout + String(chunk)).slice(-OUTPUT_LIMIT); };
    const onStderr = (chunk) => { stderr = (stderr + String(chunk)).slice(-OUTPUT_LIMIT); };
    const finish = (failure) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(killTimer);
      if (child.stdout) child.stdout.removeListener("data", onStdout);
      if (child.stderr) child.stderr.removeListener("data", onStderr);
      const output = { stdout: cleanOutput(stdout), stderr: cleanOutput(stderr) };
      if (!failure) { resolve(output); return; }
      const details = [output.stderr, output.stdout].filter(Boolean).join("\n");
      const error = new Error(`${label}: ${failure.message}${details ? `\n${details}` : ""}`);
      error.code = failure.code || "PROTON_PREFIX_COMMAND_FAILED";
      error.stdout = output.stdout;
      error.stderr = output.stderr;
      if (failure.exitCode !== undefined) error.exitCode = failure.exitCode;
      if (failure.cause) error.cause = failure.cause;
      reject(error);
    };
    const timeoutFailure = () => ({ code: "PROTON_PREFIX_COMMAND_TIMEOUT", message: `превышено время ожидания (${Math.round(timeoutMs / 1000)} с)` });
    child.once("error", (error) => finish({ message: error.message, code: error.code, cause: error }));
    child.once("close", (code, signal) => finish(timedOut ? timeoutFailure() : code === 0 ? null : {
      message: `завершился с кодом ${code == null ? signal : code}`,
      exitCode: code,
    }));
    if (child.stdout) child.stdout.on("data", onStdout);
    if (child.stderr) child.stderr.on("data", onStderr);
    timer = setTimeout(() => {
      timedOut = true;
      terminate(child, "SIGTERM");
      if (settled) return;
      killTimer = setTimeout(() => {
        terminate(child, "SIGKILL");
        finish(timeoutFailure());
        if (child.stdout && typeof child.stdout.destroy === "function") child.stdout.destroy();
        if (child.stderr && typeof child.stderr.destroy === "function") child.stderr.destroy();
      }, killGraceMs);
    }, timeoutMs);
  });
}

module.exports = { waitForPrefixCommand, OUTPUT_LIMIT, DEFAULT_TIMEOUT_MS, DEFAULT_KILL_GRACE_MS };
