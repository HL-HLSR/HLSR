"use strict";

const fs = require("fs");

// Заголовки демо. Движок определяется по сигнатуре, а не по расширению.
// Читается только заголовок (и у GoldSrc — директория сегментов по смещению
// из заголовка), сам поток демо не трогается.
//
// GoldSrc (544 байта): "HLDEMO\0\0", demoProtocol i32, netProtocol i32,
// map[260], gameDir[260], mapCRC i32, directoryOffset i32. Директория:
// count i32 и записи по 92 байта: type i32, description[64], flags i32,
// cdTrack i32, trackTime f32, frames i32, offset i32, length i32.
//
// Source (1072 байта): "HL2DEMO\0", demoProtocol i32, netProtocol i32,
// server[260], client[260], map[260], gameDir[260], playbackTime f32,
// ticks i32, frames i32, signonLength i32.

const GOLDSRC_MAGIC = Buffer.from("HLDEMO\0", "latin1");
const SOURCE_MAGIC = Buffer.from("HL2DEMO\0", "latin1");
const GOLDSRC_HEADER = 544;
const SOURCE_HEADER = 1072;
const DIR_ENTRY = 92;
const MAX_DIR_ENTRIES = 1024;
const MAX_SECONDS = 365 * 24 * 3600;

function text(buffer, start, length) {
  const raw = buffer.subarray(start, start + length);
  const end = raw.indexOf(0);
  // Управляющие символы (и битый UTF-8) из чужого файла в интерфейс не пропускаются.
  return raw
    .subarray(0, end < 0 ? length : end)
    .toString("utf8")
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x1f\x7f�]/g, "")
    .trim();
}

const sane = (value) => Number.isFinite(value) && value >= 0 && value <= MAX_SECONDS;
const round = (value) => Math.round(value * 1000) / 1000;
const fail = (code) => ({ ok: false, code });

function goldsrcGame(gameDir, demoProtocol) {
  if (!/^valve(_won)?$/i.test(gameDir)) return { game: null, supported: false, reason: "DEMO_UNSUPPORTED" };
  if (demoProtocol !== 5) return { game: "hl1", supported: false, reason: "DEMO_PROTOCOL_UNSUPPORTED" };
  return { game: "hl1", supported: true, reason: null };
}

function sourceGame(gameDir, demoProtocol) {
  const dir = gameDir.toLowerCase();
  if (dir === "ghostingmod")
    return demoProtocol === 3
      ? { game: "ghosting", supported: true, reason: null }
      : { game: "ghosting", supported: false, reason: "DEMO_PROTOCOL_UNSUPPORTED" };
  // Старые HL2 (2707/4260) и прочие моды показываются, но пока без рендера.
  return { game: dir === "hl2" ? "hl2" : null, supported: false, reason: "DEMO_UNSUPPORTED" };
}

// Директория GoldSrc: null — директории нет или она не сходится с файлом.
function goldsrcDirectory(offset, size, read) {
  if (!Number.isSafeInteger(offset) || offset < GOLDSRC_HEADER || offset + 4 > size) return null;
  const countBuffer = read(offset, 4);
  if (countBuffer.length < 4) return null;
  const count = countBuffer.readInt32LE(0);
  if (count < 1 || count > MAX_DIR_ENTRIES || offset + 4 + count * DIR_ENTRY > size) return null;
  const table = read(offset + 4, count * DIR_ENTRY);
  if (table.length < count * DIR_ENTRY) return null;
  const entries = [];
  const spans = [];
  for (let i = 0; i < count; i++) {
    const at = i * DIR_ENTRY;
    const entry = {
      type: table.readInt32LE(at),
      description: text(table, at + 4, 64),
      duration: table.readFloatLE(at + 76),
      frames: table.readInt32LE(at + 80),
    };
    const dataOffset = table.readInt32LE(at + 84);
    const dataLength = table.readInt32LE(at + 88);
    if (!sane(entry.duration) || entry.frames < 0 || dataOffset < GOLDSRC_HEADER || dataLength < 0 ||
        dataOffset + dataLength > offset)
      return null;
    entry.duration = round(entry.duration);
    entries.push(entry);
    spans.push([dataOffset, dataOffset + dataLength]);
  }
  // Записи не пересекаются (как в goldsrc-demo.js): подделка с тысячей записей
  // на один участок не множит длительность.
  spans.sort((a, b) => a[0] - b[0]);
  for (let i = 1; i < spans.length; i++) if (spans[i - 1][1] > spans[i][0]) return null;
  return entries;
}

// head — первые до 1072 байт файла, size — его длина, read(pos, len) — чтение
// с позиции (для директории GoldSrc).
function parseHeader(head, size, read) {
  if (head.length >= GOLDSRC_MAGIC.length && head.subarray(0, GOLDSRC_MAGIC.length).equals(GOLDSRC_MAGIC)) {
    if (head.length < GOLDSRC_HEADER) return fail("DEMO_TRUNCATED");
    const demoProtocol = head.readInt32LE(8);
    const netProtocol = head.readInt32LE(12);
    const map = text(head, 16, 260);
    const gameDir = text(head, 276, 260);
    const entries = goldsrcDirectory(head.readInt32LE(540), size, read);
    const game = goldsrcGame(gameDir, demoProtocol);
    const result = Object.assign(
      { ok: true, engine: "goldsrc", demoProtocol, netProtocol, map, gameDir, size },
      game,
      { duration: entries ? round(entries.reduce((sum, entry) => sum + entry.duration, 0)) : null, entries: entries || [] }
    );
    // Без директории GoldSrc демо не проигрывает (запись оборвалась).
    if (!entries && result.supported) Object.assign(result, { supported: false, reason: "DEMO_DIRECTORY_INVALID" });
    return result;
  }

  if (head.length >= SOURCE_MAGIC.length && head.subarray(0, SOURCE_MAGIC.length).equals(SOURCE_MAGIC)) {
    if (head.length < SOURCE_HEADER) return fail("DEMO_TRUNCATED");
    const playbackTime = head.readFloatLE(1056);
    const ticks = head.readInt32LE(1060);
    const frames = head.readInt32LE(1064);
    const signon = head.readInt32LE(1068);
    if (signon < 0 || SOURCE_HEADER + signon > size) return fail("DEMO_TRUNCATED");
    if (!sane(playbackTime) || ticks < 0 || frames < 0) return fail("DEMO_HEADER_INVALID");
    const demoProtocol = head.readInt32LE(8);
    const gameDir = text(head, 796, 260);
    // Незакрытая демо (вылет игры) хранит нули и ни одного кадра: длительность
    // неизвестна. Закрытый пустой сегмент (мгновенный сейвлоуд) — 0 тиков, 1 кадр.
    const duration = playbackTime > 0 ? round(playbackTime) : frames > 0 ? 0 : null;
    return Object.assign(
      {
        ok: true,
        engine: "source",
        demoProtocol,
        netProtocol: head.readInt32LE(12),
        server: text(head, 16, 260),
        client: text(head, 276, 260),
        map: text(head, 536, 260),
        gameDir,
        size,
      },
      sourceGame(gameDir, demoProtocol),
      { duration, ticks, frames, tickrate: duration && ticks ? round(ticks / duration) : null }
    );
  }

  return fail("DEMO_NOT_A_DEMO");
}

function readAt(fd, position, length) {
  const buffer = Buffer.alloc(length);
  const read = fs.readSync(fd, buffer, 0, length, position);
  return buffer.subarray(0, read);
}

function parseDemoFile(file) {
  let fd;
  try {
    fd = fs.openSync(file, "r");
  } catch (error) {
    return fail(error.code === "ENOENT" ? "DEMO_NOT_FOUND" : "DEMO_READ_FAILED");
  }
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile()) return fail("DEMO_READ_FAILED");
    return parseHeader(readAt(fd, 0, SOURCE_HEADER), stat.size, (position, length) => readAt(fd, position, length));
  } catch (error) {
    return fail("DEMO_READ_FAILED");
  } finally {
    fs.closeSync(fd);
  }
}

function parseDemoBuffer(buffer) {
  return parseHeader(buffer.subarray(0, SOURCE_HEADER), buffer.length, (position, length) =>
    buffer.subarray(position, position + length)
  );
}

module.exports = { parseDemoFile, parseDemoBuffer, GOLDSRC_HEADER, SOURCE_HEADER, DIR_ENTRY };
