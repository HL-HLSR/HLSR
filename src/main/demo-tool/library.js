"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const demoArchive = require("../game-delivery/demo-archive");
const { assertInside } = require("../game-delivery/paths");
const { readableName } = require("../../shared/ansi-name");
const headers = require("./headers");
const playlist = require("./playlist");
const sourceRender = require("./source-render");
const sourceDemo = require("./source-demo");

// Список демо: из установленных игр (demo-archive) и добавленные вручную
// (хранятся в Store путями; несуществующие отбрасываются при чтении).
// Окно ссылается на элементы по id, пути из окна принимаются только при
// добавлении и проверяются здесь же.

const EXTENSIONS = new Set([".dem", ".ghplaylist"]);
const PLAYLIST_FILE = "run.ghplaylist";
const MAX_MANUAL = 200;
const MAX_SEGMENTS = 512;
const MAX_ADD = 512;
const MAX_SCAN = 5000;
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
// Серии демок HL1 в папке игры: <имя>_<N>.dem (bxt_autorecord пишет по демке на загрузку).
// Ghosting сюда не входит: у его ранов свои плейлисты.
const SERIES_GAMES = new Set(["hl1-won"]);
const SERIES_NAME = /^(.+)_(\d{1,6})\.dem$/i;
const MAX_MISSING = 64;

const idOf = (...parts) => crypto.createHash("sha256").update(parts.join("\0"), "utf8").digest("hex").slice(0, 16);
const round3 = (value) => Math.round(value * 1000) / 1000;
const naturalByName = (a, b) => collator.compare(path.basename(a), path.basename(b)) || collator.compare(a, b);

function isRegularFile(file) {
  try {
    const stat = fs.lstatSync(file);
    return stat.isFile() && !stat.isSymbolicLink();
  } catch (error) {
    return false;
  }
}

function mtimeOf(files) {
  let latest = 0;
  for (const file of files)
    try {
      latest = Math.max(latest, fs.statSync(file).mtimeMs);
    } catch (error) {
      /* пропал между сканированием и чтением */
    }
  return latest;
}

function entry(fields) {
  const files = fields.files;
  const key = fields.kind === "segments" ? files.slice().sort().join("|") : files.join("|");
  // name — только для показа: имя от игры в UTF-8, записанное через cp1251, читается (ansi-name.js).
  return Object.assign({ id: idOf(fields.source, fields.gameId || "", fields.kind, key.toLowerCase()) }, fields, { name: readableName(fields.name) });
}

// Плейлисты Ghosting: <demoDir>/<ран>/run.ghplaylist и плоские <demoDir>/*.ghplaylist.
function scanPlaylists(context) {
  const found = [];
  if (!context.config || !Array.isArray(context.config.demoDirs)) return found;
  for (const relative of context.config.demoDirs) {
    let dir;
    let items;
    try {
      dir = assertInside(context.installPath, path.join(context.installPath, ...String(relative).split("/")));
      items = fs.readdirSync(dir, { withFileTypes: true }).slice(0, MAX_SCAN);
    } catch (error) {
      continue;
    }
    for (const item of items) {
      if (item.isSymbolicLink()) continue;
      const full = path.join(dir, item.name);
      if (item.isFile() && path.extname(item.name).toLowerCase() === ".ghplaylist")
        found.push({ file: full, name: path.basename(item.name, path.extname(item.name)) });
      else if (item.isDirectory() && isRegularFile(path.join(full, PLAYLIST_FILE)))
        found.push({ file: path.join(full, PLAYLIST_FILE), name: item.name });
    }
  }
  return found;
}

// Серии в одной папке: две и больше демки <имя>_<N> — один набор в числовом порядке
// (2 раньше 10), пропуски номеров допустимы. Одиночная и без номера — как была.
// -> { series: [{ dir, name, demos, missing }], rest: [demo] }.
function groupSeries(demos) {
  const groups = new Map();
  const rest = [];
  for (const demo of demos) {
    const match = SERIES_NAME.exec(demo.name);
    if (!match) {
      rest.push(demo);
      continue;
    }
    const key = `${demo.dir}\0${match[1].toLowerCase()}`;
    if (!groups.has(key)) groups.set(key, { dir: demo.dir, name: match[1], demos: [] });
    groups.get(key).demos.push(Object.assign({ number: Number(match[2]) }, demo));
  }
  const series = [];
  for (const group of groups.values()) {
    if (group.demos.length < 2) {
      rest.push(...group.demos.map(({ number, ...demo }) => demo));
      continue;
    }
    group.demos.sort((a, b) => a.number - b.number || collator.compare(a.name, b.name));
    group.demos = group.demos.slice(0, MAX_SEGMENTS);
    // Нет номеров между первым (с 1, если серия не с 0) и последним.
    const present = new Set(group.demos.map((demo) => demo.number));
    const last = group.demos[group.demos.length - 1].number;
    const missing = [];
    for (let n = Math.min(1, group.demos[0].number); n <= last && missing.length < MAX_MISSING; n++) if (!present.has(n)) missing.push(n);
    series.push({ dir: group.dir, name: group.name, demos: group.demos, missing });
  }
  return { series, rest };
}

function gameEntries(contexts) {
  const entries = [];
  for (const context of contexts || []) {
    let demos = [];
    try {
      demos = demoArchive.listDemos(context.config, context.installPath);
    } catch (error) {
      demos = [];
    }
    const grouped = SERIES_GAMES.has(context.gameId) ? groupSeries(demos) : { series: [], rest: demos };
    for (const run of grouped.series)
      entries.push(entry({
        // id не зависит от числа демок: серия, которая пишется прямо сейчас, остаётся выбранной.
        id: idOf("game", context.gameId, "series", `${run.dir}/${run.name}`.toLowerCase()),
        kind: "segments",
        source: "game",
        gameId: context.gameId,
        name: run.name,
        series: true,
        missing: run.missing,
        files: run.demos.map((demo) => path.join(context.installPath, ...demo.dir.split("/"), demo.name)),
        mtimeMs: Math.max(...run.demos.map((demo) => demo.mtimeMs || 0)),
      }));
    for (const demo of grouped.rest)
      entries.push(entry({
        kind: "demo",
        source: "game",
        gameId: context.gameId,
        name: demo.name,
        files: [path.join(context.installPath, ...demo.dir.split("/"), demo.name)],
        mtimeMs: demo.mtimeMs,
      }));
    for (const run of scanPlaylists(context))
      entries.push(entry({ kind: "playlist", source: "game", gameId: context.gameId, name: run.name, files: [run.file], mtimeMs: mtimeOf([run.file]) }));
  }
  return entries;
}

// Файл записи остаётся: он на месте или недоступна сама его папка (диск
// USB или сетевой отключён) — тогда элемент виден с DEMO_NOT_FOUND. Файл
// удалён из существующей папки — запись отбрасывается.
function keep(file) {
  if (typeof file !== "string" || !path.isAbsolute(file)) return false;
  return isRegularFile(file) || !fs.existsSync(path.dirname(file));
}

// Записи Store: { kind: "file", path } и { kind: "segments", paths: [...] }.
function manualEntries(stored) {
  const entries = [];
  const cleaned = [];
  for (const record of Array.isArray(stored) ? stored.slice(0, MAX_MANUAL) : []) {
    if (record && record.kind === "file" && keep(record.path)) {
      const kind = path.extname(record.path).toLowerCase() === ".ghplaylist" ? "playlist" : "demo";
      const name = kind === "playlist" && path.basename(record.path).toLowerCase() === PLAYLIST_FILE
        ? path.basename(path.dirname(record.path))
        : path.basename(record.path);
      cleaned.push({ kind: "file", path: record.path });
      entries.push(entry({ kind, source: "manual", gameId: null, name, files: [record.path], mtimeMs: mtimeOf([record.path]) }));
    } else if (record && record.kind === "segments" && Array.isArray(record.paths)) {
      const files = record.paths.filter(keep).slice(0, MAX_SEGMENTS);
      if (!files.length) continue;
      cleaned.push({ kind: "segments", paths: files });
      entries.push(entry({
        kind: files.length > 1 ? "segments" : "demo",
        source: "manual",
        gameId: null,
        name: files.length > 1 ? `${path.basename(files[0], path.extname(files[0]))} (+${files.length - 1})` : path.basename(files[0]),
        files,
        mtimeMs: mtimeOf(files),
      }));
    }
  }
  return { entries, cleaned };
}

// Метки источника на стыках сегментов (время демо = сумма длительностей).
function boundaryMarkers(segments, kind) {
  const markers = [];
  let t = 0;
  for (let i = 0; i < segments.length; i++) {
    if (i > 0) {
      const changed = segments[i].map && segments[i].map !== segments[i - 1].map;
      markers.push({
        t: round3(t),
        type: changed ? "map" : kind === "playlist" ? "saveload" : "segment",
        label: changed ? segments[i].map : String(i + 1),
      });
    }
    if (!(segments[i].duration >= 0)) break;
    t += segments[i].duration;
  }
  return markers;
}

function fileSize(file) {
  try {
    return fs.statSync(file).size;
  } catch (error) {
    return 0;
  }
}

function segmentInfo(file, index, extra) {
  const header = headers.parseDemoFile(file);
  const base = Object.assign({ index: index + 1, name: path.basename(file), path: file }, extra || {});
  // Битый файл: размер всё равно настоящий (в списке не «0 Б»).
  if (!header.ok) return Object.assign(base, { ok: false, code: header.code, engine: null, game: null, map: null, duration: null, size: fileSize(file) });
  return Object.assign(base, {
    ok: true,
    code: header.supported ? null : header.reason,
    engine: header.engine,
    game: header.game,
    supported: header.supported,
    map: header.map,
    gameDir: header.gameDir,
    demoProtocol: header.demoProtocol,
    netProtocol: header.netProtocol,
    duration: header.duration,
    size: header.size,
    ticks: header.ticks,
    frames: header.frames,
    tickrate: header.tickrate,
    entries: header.entries,
    client: header.client,
  });
}

// Полное описание элемента: движок, игра, карты, сегменты, длительность и
// метки источника. Для списка лишнее отрезается в summary().
function describe(item) {
  const out = Object.assign({}, item, { engine: null, game: null, map: null, maps: [], duration: null, supported: false,
    reason: null, segments: [], markers: [], size: 0, incomplete: false, runTime: null });
  let segments;
  if (item.kind === "playlist") {
    const read = playlist.readPlaylist(item.files[0]);
    if (!read.ok) return Object.assign(out, { reason: read.code, failedSegment: read.segment || null });
    out.incomplete = read.incomplete;
    out.runTime = read.total;
    segments = read.segments.map((segment, index) =>
      segmentInfo(segment.path, index, { timerStart: segment.start, timerEnd: segment.end, playlistMap: segment.map, sizeMatches: segment.sizeMatches,
        sha256: segment.sha256, playlistFile: segment.file }));
    out.sourceFiles = [path.basename(item.files[0])].concat(read.segments.map((segment) => segment.name));
  } else {
    segments = item.files.map((file, index) => segmentInfo(file, index));
    out.sourceFiles = segments.map((segment) => segment.name);
  }

  out.segments = segments;
  out.size = segments.reduce((sum, segment) => sum + (segment.size || 0), 0);
  const first = segments[0] || {};
  const broken = segments.find((segment) => !segment.ok);
  const unsupported = segments.find((segment) => segment.ok && !segment.supported);
  const mixed = segments.some((segment) => segment.ok && (segment.engine !== first.engine || segment.game !== first.game));
  Object.assign(out, { engine: first.engine || null, game: first.game || null, map: first.map || null });
  for (const segment of segments) if (segment.map && out.maps[out.maps.length - 1] !== segment.map) out.maps.push(segment.map);
  const known = segments.every((segment) => segment.duration >= 0);
  const total = known ? round3(segments.reduce((sum, segment) => sum + segment.duration, 0)) : 0;
  out.duration = total > 0 ? total : null;
  // Source (Ghosting): те же метки, что даст рендер, — пустые сегменты (≤ 1 тика)
  // пропущены, одна функция с адаптером, и главы HL2 по картам. HL1: стыки сегментов набора.
  if (out.engine === "source" && !broken && !mixed) {
    const picked = sourceRender.planSegments(segments, null, 60).picked;
    out.markers = sourceRender.timelineOf(picked, null).concat(sourceDemo.chapterMarkers(picked)).sort((a, b) => a.t - b.t);
  } else out.markers = boundaryMarkers(segments, item.kind);
  // Номер сегмента — только у рана из нескольких: у одиночной демки «Сегмент 1:» лишний.
  if (broken) Object.assign(out, { reason: broken.code, failedSegment: segments.length > 1 ? broken.index : null });
  else if (mixed) out.reason = "SEGMENTS_MIXED";
  else if (unsupported) Object.assign(out, { reason: unsupported.code, failedSegment: segments.length > 1 ? unsupported.index : null });
  else out.supported = true;
  if (out.supported && out.duration === null) out.warnings = ["DURATION_UNKNOWN"];
  return out;
}

const SUMMARY_FIELDS = ["id", "kind", "source", "gameId", "name", "mtimeMs", "engine", "game", "map", "maps", "duration",
  "supported", "reason", "failedSegment", "size", "incomplete", "runTime", "warnings", "series", "missing"];

function summary(described) {
  const out = {};
  for (const key of SUMMARY_FIELDS) if (described[key] !== undefined) out[key] = described[key];
  out.path = described.files[0];
  out.segmentCount = described.segments.length;
  return out;
}

// Проверка путей из окна (перетаскивание, диалог): существует, расширение,
// заголовок. UNC и пути устройств Windows не принимаются.
function checkPath(raw, platform) {
  const name = typeof raw === "string" ? path.basename(raw).slice(0, 260) : "";
  if (typeof raw !== "string" || !raw || raw.length > 4096 || raw.includes("\0") || !path.isAbsolute(raw))
    return { ok: false, name, code: "PATH_INVALID" };
  if ((platform || process.platform) === "win32" && /^[\\/]{2}/.test(raw)) return { ok: false, name, code: "PATH_INVALID" };
  const full = path.resolve(raw);
  const ext = path.extname(full).toLowerCase();
  if (!EXTENSIONS.has(ext)) return { ok: false, name, code: "FILE_TYPE_UNSUPPORTED" };
  if (!isRegularFile(full)) return { ok: false, name, code: "DEMO_NOT_FOUND" };
  if (ext === ".ghplaylist") {
    const read = playlist.readPlaylist(full);
    return read.ok ? { ok: true, name, path: full, ext } : { ok: false, name, code: read.code };
  }
  const header = headers.parseDemoFile(full);
  return header.ok ? { ok: true, name, path: full, ext, header } : { ok: false, name, code: header.code };
}

// Новые записи Store из добавленных путей: несколько демо HL1 одним набором —
// ран из сегментов (естественный порядок по имени), остальное по одному.
function addPaths(stored, rawPaths, platform) {
  const current = Array.isArray(stored) ? stored.slice() : [];
  const rejected = [];
  const accepted = [];
  for (const raw of (Array.isArray(rawPaths) ? rawPaths : []).slice(0, MAX_ADD)) {
    const checked = checkPath(raw, platform);
    if (checked.ok) accepted.push(checked);
    else rejected.push({ name: checked.name, code: checked.code });
  }
  const known = new Set();
  for (const record of current)
    for (const file of record.kind === "segments" ? record.paths || [] : [record.path]) if (typeof file === "string") known.add(file.toLowerCase());
  const fresh = accepted.filter((item) => {
    const key = item.path.toLowerCase();
    if (known.has(key)) return false;
    known.add(key);
    return true;
  });
  const hl1 = fresh.filter((item) => item.header && item.header.engine === "goldsrc" && item.header.game === "hl1");
  const records = [];
  if (hl1.length > 1) records.push({ kind: "segments", paths: hl1.map((item) => item.path).sort(naturalByName).slice(0, MAX_SEGMENTS) });
  for (const item of fresh) if (hl1.length < 2 || !hl1.includes(item)) records.push({ kind: "file", path: item.path });
  const room = Math.max(0, MAX_MANUAL - current.length);
  for (const record of records.slice(room))
    for (const file of record.kind === "segments" ? record.paths : [record.path]) rejected.push({ name: path.basename(file), code: "LIST_FULL" });
  const added = records.slice(0, room);
  return { stored: current.concat(added), added, rejected };
}

module.exports = {
  EXTENSIONS,
  MAX_MANUAL,
  idOf,
  scanPlaylists,
  groupSeries,
  gameEntries,
  manualEntries,
  boundaryMarkers,
  describe,
  summary,
  checkPath,
  addPaths,
  naturalByName,
};
