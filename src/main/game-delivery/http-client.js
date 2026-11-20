"use strict";

const http = require("http");
const https = require("https");
const { URL } = require("url");
const { Writable } = require("stream");
const { pipeline } = require("stream/promises");
const { DeliveryError } = require("./errors");

const DEFAULT_TIMEOUT_MS = 30000;
const MAX_REDIRECTS = 5;
const USER_AGENT = "HLSR-Launcher";

function isLoopback(hostname) {
  return hostname === "127.0.0.1" || hostname === "localhost" || hostname === "::1";
}

function checkRedirect(from, to) {
  if (to.protocol !== "http:" && to.protocol !== "https:")
    throw new DeliveryError("INSECURE_REDIRECT", `Редирект на недопустимую схему: ${to.protocol}`);

  if (from.protocol === "https:" && to.protocol === "http:" && !isLoopback(to.hostname))
    throw new DeliveryError(
      "INSECURE_REDIRECT",
      `Редирект с https на http запрещён: ${to.origin}`
    );
}

function rawRequest(url, options) {
  const opts = options || {};
  const target = new URL(String(url));

  if (target.protocol !== "http:" && target.protocol !== "https:")
    throw new DeliveryError("INSECURE_REDIRECT", `Недопустимая схема: ${target.protocol}`);

  const transport = target.protocol === "https:" ? https : http;
  const timeout = opts.timeout || DEFAULT_TIMEOUT_MS;

  return new Promise((resolve, reject) => {
    const request = transport.request(
      target,
      {
        method: opts.method || "GET",
        headers: Object.assign({ "user-agent": USER_AGENT }, opts.headers || {}),
        signal: opts.signal,
      },
      (response) => {
        response.requestUrl = target;
        resolve(response);
      }
    );

    request.setTimeout(timeout, () => {
      request.destroy(new DeliveryError("NETWORK_TIMEOUT", `Таймаут запроса ${target.href}`));
    });

    request.on("error", (error) => {
      reject(
        error instanceof DeliveryError
          ? error
          : new DeliveryError("NETWORK_ERROR", `Сетевая ошибка: ${error.message}`)
      );
    });

    if (opts.onRequest) opts.onRequest(request);
    request.end();
  });
}

async function request(url, options) {
  const opts = options || {};
  let current = new URL(String(url));
  let redirects = 0;

  for (;;) {
    const response = await rawRequest(current, opts);
    const status = response.statusCode;

    if (status >= 300 && status < 400 && response.headers.location) {
      response.resume();

      if (++redirects > MAX_REDIRECTS)
        throw new DeliveryError("TOO_MANY_REDIRECTS", `Слишком много редиректов: ${url}`);

      const next = new URL(response.headers.location, current);
      checkRedirect(current, next);
      current = next;
      continue;
    }

    response.finalUrl = current;
    return response;
  }
}

async function readBody(response, limitBytes, rateLimiter) {
  const limit = limitBytes || 1024 * 1024;
  const chunks = [];
  let size = 0;
  const collector = new Writable({
    write(chunk, encoding, callback) {
      size += chunk.length;
      if (size > limit) {
        callback(new DeliveryError("HTTP_ERROR", "Ответ превысил допустимый размер"));
        return;
      }
      chunks.push(chunk);
      callback();
    },
  });
  const streams = [response];
  if (rateLimiter) streams.push(rateLimiter.createStream());
  streams.push(collector);
  try {
    await pipeline(...streams);
  } catch (error) {
    throw error instanceof DeliveryError
      ? error : new DeliveryError("NETWORK_ERROR", `Обрыв ответа: ${error.message}`);
  }
  return Buffer.concat(chunks);
}

async function getJson(url, options) {
  const response = await request(url, options);
  const body = await readBody(response, (options && options.limitBytes) || 256 * 1024);

  if (response.statusCode === 401 || response.statusCode === 403)
    throw new DeliveryError("PROVIDER_FORBIDDEN", `Доступ запрещён: HTTP ${response.statusCode}`);

  if (response.statusCode < 200 || response.statusCode >= 300)
    throw new DeliveryError("HTTP_ERROR", `HTTP ${response.statusCode} для ${url}`);

  try {
    return JSON.parse(body.toString("utf8"));
  } catch (e) {
    throw new DeliveryError("PROVIDER_LINK_FAILED", `Ответ не является JSON: ${e.message}`);
  }
}

module.exports = {
  DEFAULT_TIMEOUT_MS,
  MAX_REDIRECTS,
  request,
  rawRequest,
  readBody,
  getJson,
  checkRedirect,
  isLoopback,
};
