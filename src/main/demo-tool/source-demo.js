"use strict";

const fs = require("fs");

const { DemoError } = require("./errors");
const { SOURCE_HEADER } = require("./headers");

// Главы HL2 и финиш рана Ghosting — метки из карт сегментов и из самих демок.
//
// Главы: карты chapter*.cfg игры (ghostingmod/cfg; HL2 — те же), названия — в окне
// (hl2/resource/hl2_*.txt). Глава ставится на первом сегменте с её картой; повторный
// заход на ту же карту (сейвлоуд) новой метки не даёт. Титры (chapter14, credits) — не глава.
//
// Финиш: данные мода в демке (GHRD v1, gh2013 tools/demo-recording/GHRD_SPEC.md) —
// команды «_ghrd <base64url>» в dem_consolecmd. На триггере финиша (d3_breen_01) мод
// останавливает таймер и в том же кадре пишет запись TIMER с event 2 (run finish),
// следом RUN_END с finalTime. Таймер в TIMER уже остановлен и читается нулём, поэтому
// время финиша — finalTime из RUN_END. Остановка рана без финиша (gh_stop, выход)
// пишет только RUN_END — метки нет. Подпись MAC не проверяется: метка не вердикт о ране.
//
// Формат кадров — протокол демо 3 (Source 2007/2013 SP, Ghosting): cmd u8, tick
// i32; packet/signon — cmdinfo 76 байт, seqIn, seqOut, длина и данные; consolecmd,
// datatables, stringtables — длина и данные; usercmd — номер, длина и данные.

const CHAPTERS = Object.freeze({
  d1_trainstation_01: "1",
  d1_trainstation_05: "2",
  d1_canals_01: "3",
  d1_canals_06: "4",
  d1_eli_01: "5",
  d1_town_01: "6",
  d2_coast_01: "7",
  d2_coast_09: "8",
  d2_prison_02: "9",
  d2_prison_06: "9a",
  d3_c17_02: "10",
  d3_c17_09: "11",
  d3_citadel_01: "12",
  d3_breen_01: "13",
});

const PROTOCOL = 3;
const MAX_BYTES = 256 * 1024 * 1024;
const FRAME = { signon: 1, packet: 2, synctick: 3, consolecmd: 4, usercmd: 5, datatables: 6, stop: 7, stringtables: 8 };
const CMDINFO = 76;
const GHRD = { version: 1, chunkHeader: 7, maxParts: 32, timer: 0x07, runEnd: 0x08, finish: 2 };
// Незаконченные записи (части ещё не пришли) — не больше этого, остальное отбрасывается.
const MAX_PENDING = 64;
const round3 = (value) => Math.round(value * 1000) / 1000;

// Команды консоли демки по порядку: [{ tick, text }]. broken — файл оборвался.
function consoleCommands(buffer) {
  const commands = [];
  let at = SOURCE_HEADER;
  while (at < buffer.length) {
    const cmd = buffer[at];
    // dem_stop в конце файла бывает без полного тика.
    if (cmd === FRAME.stop) return { commands, broken: false };
    if (at + 5 > buffer.length) break;
    const tick = buffer.readInt32LE(at + 1);
    at += 5;
    if (cmd === FRAME.synctick) continue;
    if (cmd === FRAME.signon || cmd === FRAME.packet) at += CMDINFO + 8;
    else if (cmd === FRAME.usercmd) at += 4;
    else if (cmd !== FRAME.consolecmd && cmd !== FRAME.datatables && cmd !== FRAME.stringtables) break;
    if (at + 4 > buffer.length) break;
    const length = buffer.readInt32LE(at);
    if (length < 0 || at + 4 + length > buffer.length) break;
    at += 4;
    if (cmd === FRAME.consolecmd) commands.push({ tick, text: buffer.toString("latin1", at, at + length).replace(/\0[\s\S]*$/, "") });
    at += length;
  }
  return { commands, broken: true };
}

// Записи GHRD из команд «_ghrd»: части собираются по (seq, part). -> [{ tick, type, body }],
// tick — кадр последней части.
function ghrdRecords(commands) {
  const pending = new Map();
  const records = [];
  for (const { tick, text } of commands) {
    if (!text.startsWith("_ghrd ")) continue;
    const chunk = Buffer.from(text.slice(6).trim(), "base64url");
    if (chunk.length <= GHRD.chunkHeader || chunk[0] !== GHRD.version) continue;
    const seq = chunk.readUInt32LE(1);
    const part = chunk[5];
    const parts = chunk[6];
    if (!parts || parts > GHRD.maxParts || part >= parts) continue;
    if (!pending.has(seq)) {
      if (pending.size >= MAX_PENDING) pending.delete(pending.keys().next().value);
      pending.set(seq, { parts, chunks: [] });
    }
    const entry = pending.get(seq);
    if (entry.parts !== parts) continue;
    entry.chunks[part] = chunk.subarray(GHRD.chunkHeader);
    if (entry.chunks.filter(Boolean).length < parts) continue;
    pending.delete(seq);
    const record = Buffer.concat(entry.chunks);
    if (record.length < 3) continue;
    const length = record.readUInt16LE(1);
    if (record.length < 3 + length) continue;
    records.push({ tick, type: record[0], body: record.subarray(3, 3 + length) });
  }
  return records;
}

// buffer — весь файл демки. -> { ok, finish: { tick, time } | null, broken }.
function scanFinish(buffer) {
  if (buffer.length < SOURCE_HEADER || buffer.toString("latin1", 0, 8) !== "HL2DEMO\0") return { ok: false, finish: null, broken: true };
  if (buffer.readInt32LE(8) !== PROTOCOL) return { ok: false, finish: null, broken: false };
  const { commands, broken } = consoleCommands(buffer);
  let finish = null;
  for (const record of ghrdRecords(commands)) {
    if (record.type === GHRD.timer && record.body.length >= 9 && record.body[4] === GHRD.finish) {
      const timer = record.body.readFloatLE(5);
      finish = { tick: record.tick, time: timer > 0 ? round3(timer) : null };
    } else if (record.type === GHRD.runEnd && finish && record.body.length >= 8) {
      const final = record.body.readFloatLE(4);
      if (final > 0) finish.time = round3(final);
    }
  }
  return { ok: true, finish, broken };
}

async function scanFile(file, options) {
  const signal = options && options.signal;
  const stat = await fs.promises.stat(file);
  if (!stat.isFile() || stat.size > MAX_BYTES) throw new DemoError("DEMO_TOO_LARGE");
  return scanFinish(await fs.promises.readFile(file, { signal }));
}

// «1:23:45.678» или «3:05.250» — время таймера рана.
function runClock(seconds) {
  const ms = Math.round(Math.max(0, seconds) * 1000);
  const hours = Math.floor(ms / 3600000);
  const minutes = Math.floor((ms % 3600000) / 60000);
  const rest = ((ms % 60000) / 1000).toFixed(3).padStart(6, "0");
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
}

// Метки глав во времени демо; picked — sourceRender.planSegments(...).picked. label — номер главы.
function chapterMarkers(picked) {
  const seen = new Set();
  const markers = [];
  for (const item of picked || []) {
    const map = String(item.segment.map || item.segment.playlistMap || "").toLowerCase();
    const id = Object.prototype.hasOwnProperty.call(CHAPTERS, map) ? CHAPTERS[map] : null;
    if (!id || seen.has(id)) continue;
    seen.add(id);
    markers.push({ t: round3(item.start), type: "chapter", label: id });
  }
  return markers;
}

// Метка финиша: тик записи TIMER finish в своём сегменте (по тикрейту заголовка), подпись —
// время таймера. Незаконченный ран (incomplete плейлиста) метки не получает.
function finishMarkers(picked, scans, options) {
  const opts = options || {};
  if (opts.incomplete) return [];
  for (let k = (picked || []).length - 1; k >= 0; k--) {
    const scan = scans && scans[k];
    if (!scan || !scan.ok || !scan.finish) continue;
    const item = picked[k];
    const rate = item.segment.tickrate > 0 ? item.segment.tickrate : 100;
    const time = scan.finish.time !== null ? scan.finish.time : opts.runTime;
    if (!(time > 0)) return [];
    return [{ t: round3(Math.min(item.end, item.start + Math.max(0, scan.finish.tick) / rate)), type: "finish", label: runClock(time) }];
  }
  return [];
}

module.exports = { CHAPTERS, PROTOCOL, FRAME, consoleCommands, ghrdRecords, scanFinish, scanFile, runClock, chapterMarkers, finishMarkers };
