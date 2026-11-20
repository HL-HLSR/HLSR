"use strict";

// Node проверяет сертификаты по своему вшитому списку и не знает о системных
// прокси, а Chromium в Electron берёт и то, и другое из системы. Если сеть
// подменяет сертификат (антивирус, корпоративный прокси, фильтр провайдера),
// Node падает с TLS-ошибкой там, где браузер открывает адрес. Поэтому
// небольшие запросы метаданных при TLS-ошибке повторяются через Chromium.

const TLS_CODE = /^ERR_TLS|^ERR_SSL|UNABLE_TO_VERIFY|UNABLE_TO_GET_ISSUER|CERT_|DEPTH_ZERO|SELF_SIGNED|EPROTO/i;

function isTlsError(error) {
  const code = error && error.code ? String(error.code) : "";
  const message = error && error.message ? String(error.message) : "";
  return TLS_CODE.test(code) || /TLS|SSL|certificate/i.test(message);
}

function electronNet() {
  try {
    const electron = require("electron");
    return electron && electron.net && typeof electron.net.fetch === "function" ? electron.net : null;
  } catch (error) {
    return null;
  }
}

async function chromiumGet(url, options) {
  const net = electronNet();
  if (!net) throw Object.assign(new Error("Сетевой стек Chromium недоступен"), { code: "NO_CHROMIUM_NET" });

  const opts = options || {};
  const response = await net.fetch(url, {
    headers: opts.headers || {},
    signal: opts.signal,
    redirect: "follow",
    cache: "no-store",
  });
  const limit = opts.limit || 2 * 1024 * 1024;
  const reader = response.body ? response.body.getReader() : null;
  const chunks = [];
  let total = 0;
  let truncated = false;

  while (reader) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > limit) {
      truncated = true;
      await reader.cancel();
      break;
    }
    chunks.push(Buffer.from(value));
  }

  const headers = {};
  response.headers.forEach((value, key) => { headers[key] = value; });
  return {
    status: response.status,
    url: response.url || url,
    headers,
    body: truncated ? null : Buffer.concat(chunks),
    truncated,
  };
}

module.exports = { isTlsError, chromiumGet, electronNet };
