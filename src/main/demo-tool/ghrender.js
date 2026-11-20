"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { DemoError } = require("./errors");
const { clockLabel } = require("./source-render");

// Режим рендера мода Ghosting «-ghrender» (протокол журнала GHRENDER 2): мод
// сам проигрывает ран или диапазон его записей (-ghrender_segments N-M), отдаёт
// только кадры рана (f_NNNN.tga), звук без сдвига (audio.wav) и журнал
// ghrender.log с событиями в номерах выходных кадров. Здесь — чистая логика: умеет
// ли мод режим, аргументы, разбор журнала, таймлайн и вырезанное во времени демо.
// Процессы — ghosting.js.

// Протокол 1 (без диапазона) в выпуски не попал: лаунчер его не принимает.
const PROTOCOL = "GHRENDER 2";
const round3 = (value) => Math.round(value * 1000) / 1000;

// Что умеет установленный мод — по строкам параметров в его bin/client.dll, а не
// по номеру выпуска (выпуски можно переставить). Файл ~5,5 МБ, больше 64 МиБ — не он.
// Кэш по размеру, mtime и хэшу начала и конца: robocopy и 7z сохраняют mtime при замене.
const MAX_CLIENT_BYTES = 64 * 1024 * 1024;
const EDGE_BYTES = 64 * 1024;
const capsCache = new Map();
function clientCaps(modDir) {
  const file = path.join(modDir, "bin", "client.dll");
  const none = { ghrender: false, segments: false };
  let fd;
  try {
    fd = fs.openSync(file, "r");
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.size > MAX_CLIENT_BYTES) return none;
    const read = (length, position) => {
      const buffer = Buffer.alloc(length);
      return buffer.subarray(0, fs.readSync(fd, buffer, 0, length, position));
    };
    const edge = Math.min(EDGE_BYTES, stat.size);
    const edges = crypto.createHash("sha256").update(read(edge, 0)).update(read(edge, stat.size - edge)).digest("hex");
    const key = `${file}|${stat.size}|${stat.mtimeMs}|${edges}`;
    if (!capsCache.has(key)) {
      const data = read(stat.size, 0);
      if (capsCache.size >= 8) capsCache.clear();
      capsCache.set(key, { ghrender: data.includes("-ghrender_out"), segments: data.includes("-ghrender_segments") });
    }
    return capsCache.get(key);
  } catch (error) {
    return none;
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

// Имя рана для -ghrender — в тех формах, которые принимает мод (gh_playdemo):
// «demos/<папка>/run.ghplaylist» или «<имя>.ghplaylist» из demos/. Части — только
// [A-Za-z0-9_.-] без «..» (в командную строку не попадёт ничего другого).
// Плейлист вне demos/ мода — null: такой ран идёт старым путём.
function playlistArg(modDir, file) {
  const rel = path.relative(path.join(modDir, "demos"), file);
  if (!rel || path.isAbsolute(rel)) return null;
  const parts = rel.split(path.sep);
  const ok = (part, max) => part.length <= max && /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(part) && !part.includes("..");
  if (parts.length === 2 && parts[1].toLowerCase() === "run.ghplaylist" && ok(parts[0], 50)) return `demos/${parts[0]}/run.ghplaylist`;
  if (parts.length === 1 && /\.ghplaylist$/i.test(parts[0]) && ok(parts[0], 149)) return parts[0];
  return null;
}

// Мод открывает сегменты по путям из записей плейлиста, а лаунчер — рядом с
// плейлистом. Совпадают, только если папку рана не переименовали и не скопировали
// под другим именем: каждая запись — «<папка плейлиста>/<имя сегмента>».
function entriesInPlace(arg, segments) {
  const dir = arg.includes("/") ? arg.slice(0, arg.lastIndexOf("/") + 1) : "demos/";
  return segments.length > 0 && segments.every((segment) => typeof segment.playlistFile === "string" &&
    segment.playlistFile.toLowerCase() === `${dir}${segment.name}`.toLowerCase());
}

// Записи плейлиста (с 1, пустые тоже считаются — как entry= в журнале), которые
// задевает интервал: «N» или «N-M»; null — все (клип задевает все непустые записи).
function segmentsArg(picked, wholeCount) {
  if (!picked.length || picked.length === wholeCount) return null;
  const first = picked[0].index + 1;
  const last = picked[picked.length - 1].index + 1;
  return first === last ? String(first) : `${first}-${last}`;
}

// Плавный вид в демке: движок не считает рывок камеры телепортом и не сбрасывает интерполяцию.
const SMOOTH_VIEW = ["+demo_avellimit", "99999", "+demo_interplimit", "99999"];

// config — аргументы source-render.configArguments (стандартный конфиг или HUD игрока).
function gameArguments({ width, height, fps, keyhud, integrity, playlist, out, segments, config }) {
  return ["-steam", "-game", "ghostingmod", "-windowed", "-noborder", "-w", String(width), "-h", String(height), "-novid", "-multirun",
    "-ghrender", playlist, "-ghrender_out", out, "-ghrender_fps", String(fps), ...(segments ? ["-ghrender_segments", segments] : []),
    "-ghrender_keyshud", keyhud ? "1" : "0", "-ghrender_intact", integrity ? "1" : "0",
    // Консоль рендера — в папке задания, не в журнал игрока (con_logfile из его autoexec).
    "+con_logfile", `${out}/console.log`,
    // Громкость попадает в audio.wav; у игрока бывает 0,05.
    "+volume", "0.8",
    ...SMOOTH_VIEW,
    ...(config || [])];
}

// «КЛЮЧ имя=значение …»; первая строка — «GHRENDER <версия>». Неизвестные ключи пропускает читатель.
function parseLine(line) {
  const text = String(line || "").replace(/\r$/, "").trim();
  if (!text) return null;
  const [key, ...rest] = text.split(/\s+/);
  const fields = {};
  for (const part of rest) {
    const at = part.indexOf("=");
    if (at > 0) fields[part.slice(0, at)] = part.slice(at + 1);
  }
  return { key, fields, text: text.slice(0, 300) };
}

const entryNumber = (value) => {
  const n = parseInt(String(value || ""), 10);
  return n > 0 ? n : null;
};

// FAIL code=… -> код лаунчера; у сегментов — номер записи плейлиста (с 1).
const FAIL_CODES = {
  BAD_OPTION: "BAD_OPTION",
  PLAYLIST_INVALID: "PLAYLIST_INVALID",
  MAP_MISSING: "MAP_MISSING",
  SEGMENT_MISSING: "SEGMENT_MISSING",
  SEGMENT_CHANGED: "SEGMENT_CHANGED",
  SEGMENT_MALFORMED: "SEGMENT_MALFORMED",
  SEGMENT_UNSAFE: "SEGMENT_UNSAFE",
  DEMO_FAILED: "DEMO_OPEN_FAILED",
  CANCELLED: "GAME_ERROR",
  WRITE_FAILED: "RENDER_FAILED",
  NO_FRAMES: "RENDER_FAILED",
  INTERNAL: "RENDER_FAILED",
};
function failError(parsed) {
  const code = FAIL_CODES[parsed.fields.code] || "RENDER_FAILED";
  const segment = /^SEGMENT_/.test(code) ? entryNumber(parsed.fields.entry) : null;
  return new DemoError(code, parsed.text, segment ? { segment } : undefined);
}

// Таймлайн из журнала. Время видео = кадр/fps; время демо (как в
// library.describe: сумма длительностей сегментов) — по записи плейлиста.
// Мод отбрасывает начало сегмента, пока таймер рана не пошёл: первый кадр
// сегмента — это начало записи + (timer − start). Между показанными
// сегментами всё лишнее (отброшенные кадры, пустые записи) уходит в cuts;
// хвост после последнего кадра — тоже, чтобы конец интервала не ушёл за видео.
// -> { offset, cuts, markers } во времени демо (jobs.js переводит во время видео).
function timelineOf({ segments, timers, frames, fps, starts, total }) {
  const anchors = [];
  for (const line of segments || []) {
    const entry = entryNumber(line.entry);
    const demoStart = entry ? starts[entry - 1] : undefined;
    const frame = Number(line.frame);
    if (!Number.isFinite(demoStart) || !(frame >= 0)) continue;
    const timer = Number(line.timer);
    const start = Number(line.start);
    const gate = Number.isFinite(timer) && Number.isFinite(start) ? Math.max(0, timer - start) : 0;
    const m = frame / fps;
    const prev = anchors[anchors.length - 1];
    // Мод оставил больше кадров, чем длилась запись: время демо не идёт назад.
    const d = Math.max(demoStart + gate, prev ? prev.d + (m - prev.m) : 0);
    anchors.push({ m, d, line, entry });
  }
  if (!anchors.length) return { offset: 0, cuts: [], markers: [] };
  const demoAt = (m) => {
    let anchor = anchors[0];
    for (const item of anchors) if (item.m <= m) anchor = item;
    return anchor.d + (m - anchor.m);
  };
  const cuts = [];
  for (let k = 1; k < anchors.length; k++) {
    const from = anchors[k - 1].d + (anchors[k].m - anchors[k - 1].m);
    if (anchors[k].d - from > 0.0005) cuts.push({ from: round3(from), to: round3(anchors[k].d) });
  }
  const end = demoAt(frames / fps);
  if (Number.isFinite(total) && total - end > 0.0005) cuts.push({ from: round3(end), to: round3(total) });

  const markers = [];
  const timerAt = new Set();
  anchors.forEach((anchor, k) => {
    const t = round3(anchor.d);
    const { line } = anchor;
    if (k === 0) markers.push({ t, type: "map", label: line.map || "" });
    else if (line.kind === "load") markers.push({ t, type: "saveload", label: String(anchor.entry) });
    else markers.push({ t, type: "map", label: line.map || "" });
    const shown = Number.isFinite(Number(line.timer)) ? Number(line.timer) : Number(line.start);
    if (Number.isFinite(shown)) {
      markers.push({ t, type: "timer", label: clockLabel(shown) });
      timerAt.add(t);
    }
  });
  for (const line of timers || []) {
    // Финиш: таймер в записи мода уже остановлен (0) — метку финиша ставит source-demo.js по RUN_END.
    if (line.event === "finish") continue;
    const t = round3(demoAt(Number(line.frame) / fps));
    const value = Number(line.timer);
    if (Number.isFinite(t) && Number.isFinite(value) && !timerAt.has(t)) markers.push({ t, type: "timer", label: clockLabel(value) });
  }
  return { offset: round3(anchors[0].d), cuts, markers: markers.sort((a, b) => a.t - b.t) };
}

// Интервал начинается внутри карты: метка карты в его начале, как у старого
// пути (source-render timelineOf), иначе jobs.js отбросит её вместе с началом.
function withRangeStart(markers, range) {
  const from = range && Number(range.from);
  if (!(from > 0)) return markers;
  let label = null;
  for (const marker of markers) if (marker.type === "map" && marker.t <= from + 1e-6) label = marker.t >= from - 1e-6 ? null : marker.label;
  return label === null ? markers : [{ t: round3(from), type: "map", label }, ...markers].sort((a, b) => a.t - b.t);
}

module.exports = { PROTOCOL, MAX_CLIENT_BYTES, clientCaps, playlistArg, entriesInPlace, segmentsArg, gameArguments, parseLine, failError, timelineOf, withRangeStart, FAIL_CODES };
