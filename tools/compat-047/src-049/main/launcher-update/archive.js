"use strict";

const fs = require("./fs-compat");
const path = require("path");
const zlib = require("zlib");

function octal(buffer) {
  const text = buffer.toString("ascii").replace(/\0.*$/, "").trim();
  if (!/^[0-7]*$/.test(text)) throw new Error("Недопустимое число в tar");
  const value = parseInt(text || "0", 8);
  if (!Number.isSafeInteger(value)) throw new Error("Слишком большое поле tar");
  return value;
}

function entryPath(header) {
  const field = (start, size) => header.subarray(start, start + size).toString("utf8").replace(/\0.*$/, "");
  const name = [field(345, 155), field(0, 100)].filter(Boolean).join("/");
  // eslint-disable-next-line no-control-regex
  if (name.startsWith("/") || /[\\\x00-\x1f\x7f]/.test(name)) throw new Error("Небезопасный путь в tar");
  const parts = name.replace(/^\.\//, "").split("/").filter(Boolean);
  if (parts.some((part) => part === ".." || part === ".")) {
    if (name === "./" || name === ".") return "";
    throw new Error("Выход за каталог установки в tar");
  }
  return parts.join("/");
}

// Our publisher emits USTAR with ordinary files/directories only. Reject links,
// special files and extended headers instead of extracting them with a shell.
async function extract(archive, destination, options = {}) {
  const limit = options.maxBytes || 2 * 1024 * 1024 * 1024;
  const input = fs.createReadStream(archive);
  const gzip = zlib.createGunzip();
  input.on("error", (error) => gzip.destroy(error));
  input.pipe(gzip);
  let pending = Buffer.alloc(0), remaining = 0, padding = 0, fd = null;
  let total = 0, entries = 0, zeros = 0;
  const files = new Set();
  try {
    for await (const chunk of gzip) {
      if (options.signal && options.signal.aborted) throw new Error("Распаковка отменена");
      pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
      while (pending.length) {
        if (remaining) {
          const size = Math.min(remaining, pending.length);
          let offset = 0;
          while (offset < size) offset += fs.writeSync(fd, pending, offset, size - offset);
          remaining -= size;
          pending = pending.subarray(size);
          if (!remaining) { fs.closeSync(fd); fd = null; }
          continue;
        }
        if (padding) {
          const size = Math.min(padding, pending.length);
          padding -= size;
          pending = pending.subarray(size);
          continue;
        }
        if (pending.length < 512) break;
        const header = pending.subarray(0, 512);
        pending = pending.subarray(512);
        if (header.every((byte) => byte === 0)) { zeros++; continue; }
        if (zeros) throw new Error("Данные после конца tar");
        const checksum = octal(header.subarray(148, 156));
        let sum = 0;
        for (let i = 0; i < 512; i++) sum += i >= 148 && i < 156 ? 32 : header[i];
        if (sum !== checksum || header.subarray(257, 262).toString() !== "ustar") throw new Error("Повреждённый заголовок USTAR");
        const type = header[156];
        if (![0, 48, 53].includes(type)) throw new Error("Ссылки и специальные файлы в tar запрещены");
        const relative = entryPath(header);
        const size = octal(header.subarray(124, 136));
        const directory = type === 53;
        if ((!relative && !directory) || (directory && size)) throw new Error("Некорректная запись tar");
        if (++entries > 50000 || (total += size) > limit) throw new Error("Превышен размер распакованного архива");
        if (files.has(relative)) throw new Error("Повторяющийся путь в tar");
        files.add(relative);
        const target = path.join(destination, relative);
        if (directory) fs.mkdirSync(target, { recursive: true, mode: 0o755 });
        else {
          fs.mkdirSync(path.dirname(target), { recursive: true, mode: 0o755 });
          const mode = octal(header.subarray(100, 108)) & 0o111 ? 0o755 : 0o644;
          fd = fs.openSync(target, "wx", mode);
          remaining = size;
          padding = (512 - size % 512) % 512;
          if (!remaining) { fs.closeSync(fd); fd = null; }
        }
        if (options.onProgress) options.onProgress({ transferred: total, total: limit });
      }
    }
    if (remaining || padding || pending.length || zeros < 2) throw new Error("Архив tar обрезан");
    if (options.expectedBytes != null && total !== options.expectedBytes) throw new Error("Размер распакованных файлов не совпал");
    return { bytes: total, entries };
  } finally {
    if (fd !== null) fs.closeSync(fd);
    input.destroy(); gzip.destroy();
  }
}

module.exports = { extract };
