"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { Readable } = require("stream");

// Узкий протокол для <video> рендерера: hlsr-media://media/<токен>. Отдаёт
// только файлы, которые главный процесс сам зарегистрировал (итоги заданий и
// предпросмотр), с поддержкой Range — без неё не работает перемотка.
const SCHEME = "hlsr-media";
const HOST = "media";
const PRIVILEGES = Object.freeze({ standard: true, secure: true, stream: true, supportFetchAPI: true });
const TYPES = { ".mp4": "video/mp4" };

function parseRange(header, size) {
  if (!header) return null;
  const match = /^bytes=(\d*)-(\d*)$/.exec(String(header).trim());
  if (!match || (!match[1] && !match[2]) || size === 0) return "invalid";
  let start;
  let end;
  if (!match[1]) {
    // bytes=-N — последние N байт.
    start = Math.max(0, size - Number(match[2]));
    end = size - 1;
  } else {
    start = Number(match[1]);
    end = match[2] ? Math.min(Number(match[2]), size - 1) : size - 1;
  }
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start > end || start >= size) return "invalid";
  return { start, end };
}

class MediaRegistry {
  constructor() {
    this.files = new Map();
  }

  register(file) {
    const token = crypto.randomBytes(16).toString("hex");
    this.files.set(token, path.resolve(file));
    return { token, url: `${SCHEME}://${HOST}/${token}` };
  }

  unregister(token) {
    return this.files.delete(token);
  }

  clear() {
    this.files.clear();
  }

  async handle(request) {
    const refuse = (status) => new Response(null, { status, headers: { "Cache-Control": "no-store" } });
    let url;
    try {
      url = new URL(request.url);
    } catch (error) {
      return refuse(400);
    }
    const token = url.pathname.replace(/^\/+/, "");
    if (url.protocol !== `${SCHEME}:` || url.hostname !== HOST || !/^[0-9a-f]{32}$/.test(token)) return refuse(404);
    if (request.method !== "GET" && request.method !== "HEAD") return refuse(405);
    const file = this.files.get(token);
    if (!file) return refuse(404);

    let size;
    try {
      const stat = fs.statSync(file);
      if (!stat.isFile()) return refuse(404);
      size = stat.size;
    } catch (error) {
      return refuse(404);
    }

    const headers = {
      "Content-Type": TYPES[path.extname(file).toLowerCase()] || "application/octet-stream",
      "Accept-Ranges": "bytes",
      "Cache-Control": "no-store",
    };
    const range = parseRange(request.headers.get("range"), size);
    if (range === "invalid")
      return new Response(null, { status: 416, headers: Object.assign(headers, { "Content-Range": `bytes */${size}` }) });
    const start = range ? range.start : 0;
    const end = range ? range.end : size - 1;
    headers["Content-Length"] = String(size ? end - start + 1 : 0);
    if (range) headers["Content-Range"] = `bytes ${start}-${end}/${size}`;
    const body = request.method === "HEAD" || size === 0 ? null : Readable.toWeb(fs.createReadStream(file, { start, end }));
    return new Response(body, { status: range ? 206 : 200, headers });
  }
}

module.exports = { SCHEME, HOST, PRIVILEGES, parseRange, MediaRegistry };
