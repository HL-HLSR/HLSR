"use strict";

const https = require("https");
const http = require("http");
const { URL } = require("url");

const MAX_RESPONSE_BYTES = 8 * 1024;

function userAgent(launcherVersion) {
  return `HLSR-Launcher/${launcherVersion || "0.0.0"}`;
}

function request(url, options) {
  const opts = options || {};
  const maxResponseBytes = Math.min(2 * 1024 * 1024, Math.max(MAX_RESPONSE_BYTES, opts.maxResponseBytes || MAX_RESPONSE_BYTES));
  const target = new URL(String(url));

  if (target.protocol !== "https:" && target.protocol !== "http:")
    return Promise.reject(new Error("TELEMETRY_BAD_SCHEME"));

  const isLoopback =
    target.hostname === "127.0.0.1" || target.hostname === "localhost" || target.hostname === "::1";
  if (target.protocol === "http:" && !isLoopback)
    return Promise.reject(new Error("TELEMETRY_INSECURE"));

  const transport = target.protocol === "https:" ? https : http;

  return new Promise((resolve, reject) => {
    const headers = Object.assign(
      {
        "user-agent": userAgent(opts.launcherVersion),
        accept: "application/json",
        "accept-encoding": "identity",
      },
      opts.headers || {}
    );

    let payload = null;
    if (opts.body !== undefined && opts.body !== null) {
      payload = Buffer.from(opts.body, "utf8");
      headers["content-type"] = "application/json";
      headers["content-length"] = String(payload.length);
    }

    const req = transport.request(
      target,
      { method: opts.method || "GET", headers },
      (response) => {
        const chunks = [];
        let size = 0;

        response.on("data", (chunk) => {
          size += chunk.length;
          if (size > maxResponseBytes) {
            response.destroy();
            return;
          }
          chunks.push(chunk);
        });

        response.on("end", () =>
          resolve({
            status: response.statusCode || 0,
            headers: response.headers || {},
            body: Buffer.concat(chunks).toString("utf8"),
          })
        );

        response.on("error", () => reject(new Error("TELEMETRY_NETWORK")));
      }
    );

    req.setTimeout(opts.timeoutMs || 15000, () => {
      req.destroy(new Error("TELEMETRY_TIMEOUT"));
    });

    req.on("error", (error) => {
      reject(error && error.message === "TELEMETRY_TIMEOUT" ? error : new Error("TELEMETRY_NETWORK"));
    });

    if (payload) req.write(payload);
    req.end();
  });
}

function retryAfterMs(headers, nowMs) {
  const raw = headers && headers["retry-after"];
  if (!raw) return null;

  const seconds = Number(String(raw).trim());
  if (Number.isFinite(seconds) && seconds >= 0) return Math.min(seconds, 3600) * 1000;

  const date = Date.parse(String(raw));
  if (!Number.isNaN(date)) {
    const delta = date - (nowMs || Date.now());
    return delta > 0 ? Math.min(delta, 3600 * 1000) : 0;
  }

  return null;
}

module.exports = { request, retryAfterMs, userAgent, MAX_RESPONSE_BYTES };
