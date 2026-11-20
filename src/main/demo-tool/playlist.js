"use strict";

const fs = require("fs");
const path = require("path");

// Плейлист рана Ghosting (demos/<ран>/run.ghplaylist или старый плоский
// demos/<имя>.ghplaylist, например gh_latest). Формат GHPL v1 из клиента
// мода (gh_demo_playlist.h): "GHPL", version u8 = 1, incomplete u8, count u16,
// total f32 (время таймера рана); записи: file str16, map str8, start f32,
// end f32 (таймер на границах сегмента), size u64, SHA-256 64 ASCII.
// Пути записей — от папки мода ("demos/myrun1/segment001.dem").

const MAX_PLAYLIST_BYTES = 1024 * 1024;
const MAX_FILES = 4096;
const MAX_SECONDS = 31536000;
const SEGMENT_PATH = /^demos\/(?:[A-Za-z0-9_-]{1,64}\/)?[A-Za-z0-9_.-]{1,149}$/;

const sane = (value) => Number.isFinite(value) && value >= 0 && value <= MAX_SECONDS;
const invalid = (reason) => ({ ok: false, code: "PLAYLIST_INVALID", reason });

function decodePlaylist(buffer) {
  if (!Buffer.isBuffer(buffer) || buffer.length < 12) return invalid("short");
  if (buffer.length > MAX_PLAYLIST_BYTES) return invalid("too-large");
  if (buffer.toString("latin1", 0, 4) !== "GHPL" || buffer[4] !== 1 || buffer[5] > 1) return invalid("magic");
  const count = buffer.readUInt16LE(6);
  const total = buffer.readFloatLE(8);
  if (!count || count > MAX_FILES || !sane(total)) return invalid("header");

  let at = 12;
  const take = (length) => {
    if (at + length > buffer.length) return null;
    const slice = buffer.subarray(at, at + length);
    at += length;
    return slice;
  };
  const string = (wide, capacity) => {
    const prefix = take(wide ? 2 : 1);
    if (!prefix) return null;
    const length = wide ? prefix.readUInt16LE(0) : prefix[0];
    const raw = length && length < capacity ? take(length) : null;
    return raw && !raw.includes(0) ? raw.toString("utf8") : null;
  };

  const entries = [];
  const names = new Set();
  // Порядок сверяется по сырым f32, как в моде; округление — после проверок.
  let previousStart = -Infinity;
  for (let i = 0; i < count; i++) {
    const file = string(true, 160);
    const map = string(false, 32);
    const numbers = take(16);
    const sha = take(64);
    if (file === null || map === null || !numbers || !sha) return invalid("entry");
    const entry = {
      file,
      map,
      start: numbers.readFloatLE(0),
      end: numbers.readFloatLE(4),
      size: Number(numbers.readBigUInt64LE(8)),
      sha256: sha.toString("latin1").toLowerCase(),
    };
    if (!/^[A-Za-z0-9_.-]{1,31}$/.test(map) || map.includes("..") || !sane(entry.start) || !sane(entry.end) ||
        entry.end < entry.start || entry.start < previousStart || entry.size < 1076 ||
        entry.size > 2 ** 40 || !/^[0-9a-f]{64}$/.test(entry.sha256))
      return invalid("entry");
    const key = file.toLowerCase();
    if (names.has(key)) return invalid("duplicate");
    names.add(key);
    previousStart = entry.start;
    entry.start = Math.round(entry.start * 1000) / 1000;
    entry.end = Math.round(entry.end * 1000) / 1000;
    entries.push(entry);
  }
  if (at !== buffer.length) return invalid("trailing");
  return { ok: true, incomplete: buffer[5] !== 0, total: Math.round(total * 1000) / 1000, entries };
}

// Сегмент из записи плейлиста — только файл в папке самого плейлиста:
// "demos/<папка>/segmentNNN.dem" -> <папка плейлиста>/segmentNNN.dem.
// Обход вверх, абсолютные пути и обратные слеши отклоняются.
function segmentLeaf(file) {
  if (typeof file !== "string" || !SEGMENT_PATH.test(file) || file.includes("..")) return null;
  const leaf = file.slice(file.lastIndexOf("/") + 1);
  return path.extname(leaf).toLowerCase() === ".dem" ? leaf : null;
}

function readPlaylist(file) {
  let buffer;
  try {
    const stat = fs.statSync(file);
    if (!stat.isFile()) return { ok: false, code: "PLAYLIST_INVALID", reason: "not-file" };
    if (stat.size > MAX_PLAYLIST_BYTES) return invalid("too-large");
    buffer = fs.readFileSync(file);
  } catch (error) {
    return { ok: false, code: error.code === "ENOENT" ? "DEMO_NOT_FOUND" : "DEMO_READ_FAILED" };
  }
  const decoded = decodePlaylist(buffer);
  if (!decoded.ok) return decoded;

  const dir = path.dirname(file);
  const segments = [];
  for (let index = 0; index < decoded.entries.length; index++) {
    const entry = decoded.entries[index];
    const leaf = segmentLeaf(entry.file);
    if (!leaf) return { ok: false, code: "PLAYLIST_PATH_ESCAPE", segment: index + 1 };
    const full = path.join(dir, leaf);
    let stat = null;
    try {
      stat = fs.lstatSync(full);
    } catch (error) {
      stat = null;
    }
    if (!stat || !stat.isFile() || stat.isSymbolicLink())
      return { ok: false, code: "SEGMENT_MISSING", segment: index + 1, name: leaf };
    segments.push(Object.assign({}, entry, { name: leaf, path: full, sizeMatches: stat.size === entry.size }));
  }
  return { ok: true, incomplete: decoded.incomplete, total: decoded.total, segments };
}

module.exports = { decodePlaylist, readPlaylist, segmentLeaf, MAX_PLAYLIST_BYTES };
