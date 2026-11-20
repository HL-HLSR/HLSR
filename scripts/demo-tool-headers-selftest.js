"use strict";

// «Демо»: заголовки GoldSrc/Source, плейлист Ghosting, список и приём файлов.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const headers = require("../src/main/demo-tool/headers");
const playlist = require("../src/main/demo-tool/playlist");
const library = require("../src/main/demo-tool/library");
const DemoTool = require("../src/main/demo-tool");
const { localConfigOf } = require("../src/main/game-delivery/manifests");
const fx = require("./lib/demo-fixtures");

function tempDir(t, prefix) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix || "hlsr-demo-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("GoldSrc: HL1 по сигнатуре, директория сегментов даёт длительность", () => {
  const parsed = headers.parseDemoBuffer(fx.goldsrcDemo());
  assert.equal(parsed.ok, true);
  assert.equal(parsed.engine, "goldsrc");
  assert.equal(parsed.game, "hl1");
  assert.equal(parsed.supported, true);
  assert.equal(parsed.map, "c1a0");
  assert.equal(parsed.gameDir, "valve_WON");
  assert.equal(parsed.demoProtocol, 5);
  assert.equal(parsed.netProtocol, 48);
  assert.equal(parsed.duration, 12.75);
  assert.deepEqual(parsed.entries.map((entry) => entry.description), ["LOADING", "Playback"]);
});

test("GoldSrc: чужой мод и другой протокол показываются, но без поддержки", () => {
  const mod = headers.parseDemoBuffer(fx.goldsrcDemo({ gameDir: "cstrike" }));
  assert.equal(mod.ok, true);
  assert.equal(mod.supported, false);
  assert.equal(mod.reason, "DEMO_UNSUPPORTED");
  const proto = headers.parseDemoBuffer(fx.goldsrcDemo({ protocol: 4 }));
  assert.equal(proto.reason, "DEMO_PROTOCOL_UNSUPPORTED");
});

test("GoldSrc: смещение директории за файлом, огромный count, оборванная запись — без падения", () => {
  for (const options of [{ directoryOffset: 10 ** 9 }, { directoryOffset: 100 }, { entryCount: 1e6 }, { directoryOffset: 0 }]) {
    const parsed = headers.parseDemoBuffer(fx.goldsrcDemo(options));
    assert.equal(parsed.ok, true, JSON.stringify(options));
    assert.equal(parsed.supported, false);
    assert.equal(parsed.reason, "DEMO_DIRECTORY_INVALID");
    assert.equal(parsed.duration, null);
  }
  // Запись директории указывает за конец файла.
  const bad = fx.goldsrcDemo();
  bad.writeInt32LE(10 ** 8, bad.length - 92 + 88);
  assert.equal(headers.parseDemoBuffer(bad).reason, "DEMO_DIRECTORY_INVALID");
});

test("Source: Ghosting по game dir, HL2 и прочие — без поддержки", () => {
  const ghosting = headers.parseDemoBuffer(fx.sourceDemo());
  assert.equal(ghosting.ok, true);
  assert.equal(ghosting.engine, "source");
  assert.equal(ghosting.game, "ghosting");
  assert.equal(ghosting.supported, true);
  assert.equal(ghosting.duration, 3.21);
  assert.equal(ghosting.ticks, 321);
  assert.equal(ghosting.frames, 313);
  assert.equal(ghosting.tickrate, 100);
  assert.equal(ghosting.client, "runner");
  const hl2 = headers.parseDemoBuffer(fx.sourceDemo({ gameDir: "hl2", protocol: 3 }));
  assert.equal(hl2.supported, false);
  assert.equal(hl2.game, "hl2");
  assert.equal(hl2.reason, "DEMO_UNSUPPORTED");
  assert.equal(headers.parseDemoBuffer(fx.sourceDemo({ gameDir: "hl2mp" })).game, null);
});

test("Source: пустой сегмент — 0 с, незакрытая запись — длительность неизвестна", () => {
  assert.equal(headers.parseDemoBuffer(fx.sourceDemo({ playback: 0, ticks: 0, frames: 1 })).duration, 0);
  assert.equal(headers.parseDemoBuffer(fx.sourceDemo({ playback: 0, ticks: 0, frames: 0 })).duration, null);
  assert.equal(headers.parseDemoBuffer(fx.sourceDemo({ playback: NaN })).code, "DEMO_HEADER_INVALID");
});

test("обрезанные и чужие файлы: понятный код вместо исключения", () => {
  assert.equal(headers.parseDemoBuffer(fx.goldsrcDemo().subarray(0, 300)).code, "DEMO_TRUNCATED");
  assert.equal(headers.parseDemoBuffer(fx.sourceDemo().subarray(0, 900)).code, "DEMO_TRUNCATED");
  // signon длиннее файла — запись оборвана.
  assert.equal(headers.parseDemoBuffer(fx.sourceDemo().subarray(0, 1100)).code, "DEMO_TRUNCATED");
  assert.equal(headers.parseDemoBuffer(Buffer.from("PK\u0003\u0004 zip")).code, "DEMO_NOT_A_DEMO");
  assert.equal(headers.parseDemoBuffer(Buffer.alloc(0)).code, "DEMO_NOT_A_DEMO");
});

test("файл: читается только заголовок, отсутствующий — DEMO_NOT_FOUND", (t) => {
  const dir = tempDir(t);
  const file = path.join(dir, "big.dem");
  // Большой файл: разбор читает только заголовок, а не файл целиком.
  const fd = fs.openSync(file, "w");
  fs.writeSync(fd, fx.sourceDemo({ signon: 10 }));
  fs.ftruncateSync(fd, 64 * 1024 * 1024);
  fs.closeSync(fd);
  const realRead = fs.readSync;
  let bytes = 0;
  fs.readSync = (...args) => {
    const read = realRead(...args);
    bytes += read;
    return read;
  };
  let parsed;
  try {
    parsed = headers.parseDemoFile(file);
  } finally {
    fs.readSync = realRead;
  }
  assert.equal(parsed.ok, true);
  assert.equal(parsed.size, 64 * 1024 * 1024);
  assert.ok(bytes <= 1072, `прочитано ${bytes} байт`);
  assert.equal(headers.parseDemoFile(path.join(dir, "none.dem")).code, "DEMO_NOT_FOUND");
  assert.equal(headers.parseDemoFile(dir).code, "DEMO_READ_FAILED");
});

test("плейлист GHPL: записи, incomplete, таймер", () => {
  const buffer = fx.playlistBuffer([
    { file: "demos/myrun9/segment001.dem", map: "d1_trainstation_01", start: 0.05, end: 3.25, size: 5000 },
    { file: "demos/myrun9/segment002.dem", map: "d1_trainstation_02", start: 3.26, end: 9.5, size: 6000 },
  ], { total: 9.5, incomplete: 1 });
  const decoded = playlist.decodePlaylist(buffer);
  assert.equal(decoded.ok, true);
  assert.equal(decoded.incomplete, true);
  assert.equal(decoded.total, 9.5);
  assert.deepEqual(decoded.entries.map((entry) => [entry.file, entry.map, entry.start, entry.end, entry.size]), [
    ["demos/myrun9/segment001.dem", "d1_trainstation_01", 0.05, 3.25, 5000],
    ["demos/myrun9/segment002.dem", "d1_trainstation_02", 3.26, 9.5, 6000],
  ]);
});

test("плейлист GHPL: равные start соседних записей (сырые f32) допустимы, как в моде", () => {
  const decoded = playlist.decodePlaylist(fx.playlistBuffer([
    { file: "demos/a/segment001.dem", map: "m", start: 12.3456, end: 12.3456, size: 5000 },
    { file: "demos/a/segment002.dem", map: "m", start: 12.3456, end: 13, size: 5000 },
  ], { total: 13 }));
  assert.equal(decoded.ok, true, JSON.stringify(decoded));
  assert.equal(decoded.entries[1].start, 12.346);
  assert.equal(playlist.decodePlaylist(fx.playlistBuffer([
    { file: "demos/a/segment001.dem", map: "m", start: 2, end: 3, size: 5000 },
    { file: "demos/a/segment002.dem", map: "m", start: 1, end: 3, size: 5000 },
  ])).code, "PLAYLIST_INVALID");
});

test("коды ошибок для окна: errno не уходит наружу, ENOSPC — NO_SPACE", () => {
  const { publicError, DemoError } = require("../src/main/demo-tool/errors");
  assert.deepEqual(publicError(Object.assign(new Error("x"), { code: "ENOSPC" })), { code: "NO_SPACE" });
  assert.deepEqual(publicError(Object.assign(new Error("x"), { code: "EACCES" })), { code: "JOB_FAILED" });
  assert.deepEqual(publicError(Object.assign(new Error("x"), { code: "ENOENT" }), "DEMO_FAILED"), { code: "DEMO_FAILED" });
  assert.deepEqual(publicError(Object.assign(new Error("x"), { code: "EXTRACT_FAILED" })), { code: "EXTRACT_FAILED" });
  assert.deepEqual(publicError(new DemoError("ENCODER_UNAVAILABLE")), { code: "ENCODER_UNAVAILABLE" });
  assert.deepEqual(publicError(new DemoError("DISCORD_TOO_LONG", null, { recommendedMaxSeconds: 5 })), { code: "DISCORD_TOO_LONG", recommendedMaxSeconds: 5 });
});

test("плейлист GHPL: битые данные — PLAYLIST_INVALID", () => {
  const good = [{ file: "demos/a/segment001.dem", map: "m", start: 0, end: 1, size: 5000 }];
  const cases = [
    Buffer.from("GHPL"),
    Buffer.concat([fx.playlistBuffer(good), Buffer.from([0])]),
    fx.playlistBuffer(good).subarray(0, 30),
    fx.playlistBuffer(good, { version: 2 }),
    fx.playlistBuffer([Object.assign({}, good[0], { end: -1 })]),
    fx.playlistBuffer([Object.assign({}, good[0], { size: 10 })]),
    fx.playlistBuffer([Object.assign({}, good[0], { sha256: "z".repeat(64) })]),
    fx.playlistBuffer([good[0], Object.assign({}, good[0], { file: "DEMOS/A/SEGMENT001.DEM" })]),
    fx.playlistBuffer([Object.assign({}, good[0], { map: "../x" })]),
    fx.playlistBuffer([]),
  ];
  for (const buffer of cases) assert.equal(playlist.decodePlaylist(buffer).code, "PLAYLIST_INVALID");
});

test("плейлист: сегменты только из его папки, выход наверх и отсутствующий сегмент — понятные ошибки", (t) => {
  const dir = tempDir(t);
  const runDir = path.join(dir, "ghostingmod", "demos", "myrun1");
  fs.mkdirSync(runDir, { recursive: true });
  fs.writeFileSync(path.join(runDir, "segment001.dem"), fx.sourceDemo());
  fs.writeFileSync(path.join(dir, "ghostingmod", "secret.dem"), fx.sourceDemo());
  const entry = (file) => ({ file, map: "d1_trainstation_01", start: 0, end: 1, size: 5000 });
  const write = (entries) => {
    const file = path.join(runDir, "run.ghplaylist");
    fs.writeFileSync(file, fx.playlistBuffer(entries));
    return playlist.readPlaylist(file);
  };

  const ok = write([entry("demos/myrun1/segment001.dem")]);
  assert.equal(ok.ok, true);
  assert.equal(ok.segments[0].path, path.join(runDir, "segment001.dem"));
  assert.equal(ok.segments[0].sizeMatches, false);

  for (const evil of ["demos/../secret.dem", "demos/myrun1/../../secret.dem", "..\\..\\secret.dem", "C:/secret.dem",
    "/etc/passwd.dem", "demos\\myrun1\\segment001.dem", "demos/a/b/segment001.dem", "demos/myrun1/segment001.txt"])
    assert.equal(write([entry(evil)]).code, "PLAYLIST_PATH_ESCAPE", evil);

  const missing = write([entry("demos/myrun1/segment001.dem"), entry("demos/myrun1/segment002.dem")]);
  assert.deepEqual(missing, { ok: false, code: "SEGMENT_MISSING", segment: 2, name: "segment002.dem" });
});

test("плейлист владельца того же формата: gh_latest (плоский) и папка рана находятся в игре", (t) => {
  const install = tempDir(t);
  const demos = path.join(install, "ghostingmod", "demos");
  fx.writeGhostingRun(demos, "myrun1", [{ map: "d1_trainstation_01", duration: 4 }, { map: "d1_trainstation_01", duration: 2 }, { map: "d1_trainstation_02", duration: 3 }]);
  // Старый плоский плейлист: demos/gh_latest.ghplaylist + demos/gh_latest_seg001.dem.
  fs.writeFileSync(path.join(demos, "gh_latest_seg001.dem"), fx.sourceDemo({ playback: 5, ticks: 500, frames: 500 }));
  fs.writeFileSync(path.join(demos, "gh_latest.ghplaylist"), fx.playlistBuffer([
    { file: "demos/gh_latest_seg001.dem", map: "d1_canals_01", start: 0, end: 5, size: fs.statSync(path.join(demos, "gh_latest_seg001.dem")).size },
  ]));
  const context = { gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") };
  const entries = library.gameEntries([context]);
  const names = entries.map((entry) => `${entry.kind}:${entry.name}`).sort();
  assert.deepEqual(names, ["demo:gh_latest_seg001.dem", "playlist:gh_latest", "playlist:myrun1"]);

  const run = library.describe(entries.find((entry) => entry.name === "myrun1"));
  assert.equal(run.supported, true);
  assert.equal(run.game, "ghosting");
  assert.equal(run.duration, 9);
  assert.deepEqual(run.maps, ["d1_trainstation_01", "d1_trainstation_02"]);
  // Те же метки, что даст рендер Ghosting (source-render.timelineOf): начало, стыки, таймер рана из плейлиста;
  // и глава Point Insertion на d1_trainstation_01.
  assert.deepEqual(run.markers, [
    { t: 0, type: "map", label: "d1_trainstation_01" },
    { t: 0, type: "timer", label: "0:00.000" },
    { t: 0, type: "chapter", label: "1" },
    { t: 4, type: "saveload", label: "2" },
    { t: 4, type: "timer", label: "0:04.010" },
    { t: 6, type: "map", label: "d1_trainstation_02" },
    { t: 6, type: "timer", label: "0:06.020" },
    { t: 9, type: "timer", label: "0:09.020" },
  ]);
  assert.deepEqual(run.sourceFiles, ["run.ghplaylist", "segment001.dem", "segment002.dem", "segment003.dem"]);
  const latest = library.describe(entries.find((entry) => entry.name === "gh_latest"));
  assert.equal(latest.supported, true);
  assert.equal(latest.duration, 5);
});

test("приём путей: заголовок, расширение, UNC; несколько HL1 — набор в естественном порядке", (t) => {
  const dir = tempDir(t);
  const file = (name, data) => {
    const full = path.join(dir, name);
    fs.writeFileSync(full, data);
    return full;
  };
  const seg10 = file("run_10.dem", fx.goldsrcDemo({ map: "c2a1" }));
  const seg2 = file("run_2.dem", fx.goldsrcDemo({ map: "c1a1" }));
  const seg1 = file("run_1.dem", fx.goldsrcDemo({ map: "c1a0" }));
  const ghost = file("ghost.dem", fx.sourceDemo());
  const fake = file("fake.dem", Buffer.from("not a demo at all"));
  const text = file("notes.txt", "x");
  const result = library.addPaths([], [seg10, seg2, seg1, ghost, fake, text, "relative.dem", "\\\\server\\share\\a.dem", path.join(dir, "gone.dem"), 42], "win32");
  assert.deepEqual(result.added, [
    { kind: "segments", paths: [seg1, seg2, seg10] },
    { kind: "file", path: ghost },
  ]);
  assert.deepEqual(result.rejected.map((item) => item.code), ["DEMO_NOT_A_DEMO", "FILE_TYPE_UNSUPPORTED", "PATH_INVALID", "PATH_INVALID", "DEMO_NOT_FOUND", "PATH_INVALID"]);
  // Повторное добавление тех же файлов ничего не дублирует.
  assert.deepEqual(library.addPaths(result.stored, [ghost, seg1], "win32").added, []);

  const manual = library.manualEntries(result.stored);
  const set = library.describe(manual.entries[0]);
  assert.equal(set.kind, "segments");
  assert.equal(set.duration, 38.25);
  assert.deepEqual(set.markers.map((marker) => [marker.t, marker.type, marker.label]), [[12.75, "map", "c1a1"], [25.5, "map", "c2a1"]]);

  // Удалённый файл отбрасывается при чтении.
  fs.rmSync(ghost);
  assert.equal(library.manualEntries(result.stored).cleaned.length, 1);
  // Файл с недоступного диска (нет самой папки) остаётся и виден с DEMO_NOT_FOUND.
  const offline = path.join(dir, "usb-unplugged", "run.dem");
  const kept = library.manualEntries([{ kind: "file", path: offline }]);
  assert.deepEqual(kept.cleaned, [{ kind: "file", path: offline }]);
  const described = library.describe(kept.entries[0]);
  assert.deepEqual([described.supported, described.reason], [false, "DEMO_NOT_FOUND"]);
});

test("DemoTool: список, добавление, порядок набора, удаление; пропавшие файлы уходят из Store", async (t) => {
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  fx.writeGhostingRun(path.join(install, "ghostingmod", "demos"), "myrun1", [{ map: "d1_trainstation_01", duration: 2 }]);
  const store = fx.memoryStore();
  const tool = new DemoTool({
    store,
    games: () => [{ gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") }],
    jobsRoot: path.join(dir, "jobs"),
    production: true,
  });
  const segA = path.join(dir, "b_2.dem");
  const segB = path.join(dir, "b_1.dem");
  fs.writeFileSync(segA, fx.goldsrcDemo({ map: "c1a1" }));
  fs.writeFileSync(segB, fx.goldsrcDemo({ map: "c1a0" }));
  const hl2 = path.join(dir, "old.dem");
  fs.writeFileSync(hl2, fx.sourceDemo({ gameDir: "hl2" }));

  const added = tool.addPaths([segA, segB, hl2]);
  assert.equal(added.status, "ok");
  assert.equal(added.added.length, 2);
  const items = tool.list().items;
  assert.deepEqual(items.map((item) => `${item.source}:${item.kind}:${item.supported}`), ["game:playlist:true", "manual:segments:true", "manual:demo:false"]);
  assert.equal(items[2].reason, "DEMO_UNSUPPORTED");

  const set = items[1];
  assert.equal(set.name, "b_1 (+1)");
  assert.equal(set.segmentCount, 2);
  assert.deepEqual((await tool.inspect(set.id)).segments.map((segment) => segment.map), ["c1a0", "c1a1"]);
  assert.deepEqual((await tool.reorder(set.id, [1, 0])).segments.map((segment) => segment.map), ["c1a1", "c1a0"]);
  assert.equal(tool.list().items[1].id, set.id, "id набора не зависит от порядка");
  await assert.rejects(() => tool.reorder(set.id, [0, 0]), { code: "ORDER_INVALID" });
  await assert.rejects(() => tool.reorder(items[0].id, [0]), { code: "DEMO_NOT_FOUND" });
  // Ghosting рендерит адаптер ghosting: в этой папке нет hl2.exe — игра не установлена (на Linux рендера нет).
  assert.equal((await tool.inspect(items[0].id)).render.code, process.platform === "win32" ? "GAME_NOT_INSTALLED" : "RENDER_NOT_READY");
  assert.equal((await tool.inspect(items[2].id)).render.code, "DEMO_UNSUPPORTED");

  fs.rmSync(hl2);
  assert.equal(tool.list().items.length, 2);
  assert.equal(store.data.manual.length, 1);
  tool.remove(set.id);
  assert.deepEqual(store.data.manual, []);
  await assert.rejects(() => tool.inspect("nope"), { code: "DEMO_NOT_FOUND" });
});

test("IPC: каналы demo-* отвечают кодом ошибки, чужой отправитель отклоняется", async (t) => {
  const dir = tempDir(t);
  const handlers = new Map();
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: path.join(dir, "jobs"), production: true });
  DemoTool.bind({ handle: (channel, fn) => handlers.set(channel, fn) }, tool, (event) => event.trusted === true);
  const preload = fs.readFileSync(path.join(__dirname, "../src/main/preload.js"), "utf8");
  for (const channel of handlers.keys()) assert.ok(preload.includes(`"${channel}"`), channel);
  const ok = { trusted: true };
  assert.deepEqual(await handlers.get("demo-list")(ok), { status: "ok", items: [] });
  assert.deepEqual(await handlers.get("demo-export-start")(ok, "missing", {}), { status: "error", code: "DEMO_NOT_FOUND" });
  assert.deepEqual(await handlers.get("demo-add-paths")(ok, "C:/x.dem"), { status: "error", code: "PATH_INVALID" });
  assert.deepEqual(await handlers.get("demo-media-url")(ok, "x"), { status: "error", code: "RESULT_NOT_FOUND" });
  assert.deepEqual(await handlers.get("demo-list")({ trusted: false }), { status: "error", code: "UNTRUSTED" });
  assert.deepEqual(await handlers.get("demo-cancel")(ok), { status: "ok", cancelled: false });
});

test("метки до рендера — как у рендера: пустые сегменты (≤ 1 тика) пропущены; битая демка — настоящий размер, без номера сегмента", (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-demo-markers-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  // Спам сейвлоуда: пачка пустых сегментов в одной точке.
  const file = fx.writeGhostingRun(path.join(dir, "demos"), "spam", [
    { map: "m1", duration: 2 }, { map: "m1", duration: 0.01 }, { map: "m1", duration: 0.01 }, { map: "m1", duration: 0.01 }, { map: "m1", duration: 3 },
  ]);
  const run = library.describe({ kind: "playlist", files: [file], name: "spam" });
  const sourceRender = require("../src/main/demo-tool/source-render");
  const adapterMarkers = sourceRender.timelineOf(sourceRender.planSegments(run.segments, null, 60).picked, null);
  assert.deepEqual(run.markers, adapterMarkers, "одна функция с адаптером");
  assert.deepEqual(run.markers.filter((marker) => marker.type === "saveload").map((marker) => marker.label), ["5"], "три пустых сегмента — без меток");

  const broken = path.join(dir, "broken.dem");
  // Обрезанный HL2DEMO (как test-demo-*.dem у проверяющего): заголовок неполный.
  fs.writeFileSync(broken, fx.sourceDemo().subarray(0, 1000));
  const single = library.describe({ kind: "demo", files: [broken], name: "broken.dem" });
  assert.equal(single.supported, false);
  assert.equal(single.size, 1000, "размер файла, а не 0");
  assert.equal(single.failedSegment, null, "у одиночной демки номер сегмента не нужен");
});
test("главы HL2: метка на первой карте главы по chapter*.cfg, повторный заход и титры — без метки", () => {
  const sourceDemo = require("../src/main/demo-tool/source-demo");
  const seg = (map, start) => ({ segment: { map }, start, end: start + 1 });
  const picked = [
    seg("d1_trainstation_01", 0), seg("d1_trainstation_02", 1), seg("d1_trainstation_05", 2), seg("d1_trainstation_05", 3),
    seg("d1_trainstation_01", 4), seg("D1_Canals_01", 5), seg("d2_prison_02", 6), seg("d2_prison_06", 7), seg("credits", 8),
  ];
  assert.deepEqual(sourceDemo.chapterMarkers(picked), [
    { t: 0, type: "chapter", label: "1" }, { t: 2, type: "chapter", label: "2" }, { t: 5, type: "chapter", label: "3" },
    { t: 6, type: "chapter", label: "9" }, { t: 7, type: "chapter", label: "9a" },
  ]);
  // Карта из плейлиста, если в заголовке её нет; 14 глав, у каждой своя карта.
  assert.deepEqual(sourceDemo.chapterMarkers([{ segment: { map: "", playlistMap: "d3_breen_01" }, start: 9 }]), [{ t: 9, type: "chapter", label: "13" }]);
  assert.equal(new Set(Object.values(sourceDemo.CHAPTERS)).size, 14);
  // Названия есть в обоих языках окна.
  const demoTool = require("../src/renderer/components/Launcher/demo-tool");
  const locales = require("../src/renderer/components/Launcher/demo-locales");
  for (const id of Object.values(sourceDemo.CHAPTERS)) {
    const key = demoTool.chapterKey(id);
    assert.ok(key && locales.ru[key] && locales.en[key], id);
  }
  assert.equal(demoTool.chapterKey("99"), null);
});

test("финиш Ghosting: запись мода TIMER finish в демке, время — из RUN_END; остановка без финиша и чужие данные — без метки", () => {
  const sourceDemo = require("../src/main/demo-tool/source-demo");
  const timer = (event, value, map) => fx.ghrdBody([["f32", 1], ["u8", event], ["f32", value], ["str8", map || "d3_breen_01"]]);
  const runEnd = (final) => fx.ghrdBody([["f32", final], ["f32", final], ["u16", 30]]);
  const scan = (commands) => sourceDemo.scanFinish(fx.sourceDemo({ body: fx.sourceFrames(commands) }));
  // Как в моде: старт, смена карты, финиш (таймер уже остановлен — 0) и RUN_END в том же кадре.
  const finished = scan([
    ...fx.ghrdCommands(0, 0x07, timer(1, 0)).map((text) => [0, text]),
    ...fx.ghrdCommands(1, 0x07, timer(3, 12.5)).map((text) => [40, text]),
    [120, "+forward 33"],
    ...fx.ghrdCommands(2, 0x07, timer(2, 0)).map((text) => [300, text]),
    ...fx.ghrdCommands(3, 0x08, runEnd(5025.678)).map((text) => [300, text]),
  ]);
  assert.deepEqual(finished, { ok: true, finish: { tick: 300, time: 5025.678 }, broken: false });
  assert.equal(sourceDemo.runClock(5025.678), "1:23:45.678");
  assert.equal(sourceDemo.runClock(185.25), "3:05.250");
  // Запись из нескольких частей собирается; без RUN_END — время из самой записи, если оно есть.
  assert.deepEqual(scan(fx.ghrdCommands(7, 0x07, timer(2, 61.5), 3).map((text) => [90, text])).finish, { tick: 90, time: 61.5 });
  assert.deepEqual(scan(fx.ghrdCommands(7, 0x07, timer(2, 0)).map((text) => [90, text])).finish, { tick: 90, time: null });
  // gh_stop или выход: только RUN_END — финиша нет. Битые и чужие части не мешают.
  assert.equal(scan([...fx.ghrdCommands(0, 0x07, timer(1, 0)).map((text) => [0, text]), ...fx.ghrdCommands(1, 0x08, runEnd(8.72)).map((text) => [130, text])]).finish, null);
  assert.equal(scan([[5, "_ghrd !!!"], [6, "_ghrd AQ"], [7, "_ghrd AgAAAAAAAQ"], [8, "setpause"]]).finish, null);
  assert.deepEqual(sourceDemo.scanFinish(fx.sourceDemo({ protocol: 4, body: fx.sourceFrames([[5, "x"]]) })), { ok: false, finish: null, broken: false });
  assert.equal(sourceDemo.scanFinish(Buffer.from("HLDEMO")).ok, false);
  // Время демо: начало сегмента + тик / тикрейт заголовка; незаконченный ран — без метки; без времени — время плейлиста.
  const picked = [{ segment: { tickrate: 100 }, start: 0, end: 2 }, { segment: { tickrate: 66.667 }, start: 2, end: 9 }];
  const scans = [{ ok: true, finish: null }, { ok: true, finish: { tick: 200, time: 5025.678 } }];
  assert.deepEqual(sourceDemo.finishMarkers(picked, scans), [{ t: 5, type: "finish", label: "1:23:45.678" }]);
  assert.deepEqual(sourceDemo.finishMarkers(picked, scans, { incomplete: true }), []);
  assert.deepEqual(sourceDemo.finishMarkers(picked, [null, { ok: true, finish: { tick: 900, time: null } }], { runTime: 61.5 }),
    [{ t: 9, type: "finish", label: "1:01.500" }]);
  assert.deepEqual(sourceDemo.finishMarkers(picked, [null, null]), []);
});

test("DemoTool: главы и финиш Ghosting — до рендера и во времени видео; незаконченный ран без финиша; HL1 без них", async (t) => {
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  const demos = path.join(install, "ghostingmod", "demos");
  const timer = (event, value) => fx.ghrdBody([["f32", 1], ["u8", event], ["f32", value], ["str8", "d3_breen_01"]]);
  const finishCommands = fx.ghrdCommands(9, 0x07, timer(2, 0)).concat(fx.ghrdCommands(10, 0x08, fx.ghrdBody([["f32", 1], ["f32", 4.75], ["u16", 3]])))
    .map((text) => [150, text]);
  const segments = [
    { map: "d1_trainstation_05", duration: 2 },
    { map: "d1_trainstation_05", duration: 1 },
    { map: "d3_breen_01", duration: 3, commands: finishCommands },
  ];
  fx.writeGhostingRun(demos, "done", segments);
  fx.writeGhostingRun(demos, "quit", segments, { incomplete: 1 });
  const tool = new DemoTool({
    store: fx.memoryStore(),
    games: () => [{ gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") }],
    jobsRoot: path.join(dir, "jobs"),
    production: true,
  });
  t.after(() => tool.dispose());
  const items = tool.list().items;
  const done = items.find((item) => item.name === "done");
  const quit = items.find((item) => item.name === "quit");
  const pick = (markers) => markers.filter((marker) => marker.type === "chapter" || marker.type === "finish" || marker.type === "map")
    .map((marker) => [marker.t, marker.type, marker.label]);
  // Карта на каждой смене, глава — на первом заходе (сейвлоуд той же карты — без неё), финиш — на тике записи.
  assert.deepEqual(pick((await tool.inspect(done.id)).markers),
    [[0, "map", "d1_trainstation_05"], [0, "chapter", "2"], [3, "map", "d3_breen_01"], [3, "chapter", "13"], [4.5, "finish", "0:04.750"]]);
  // «Дополнительные настройки» Ghosting: KeyHUD, конфиг и HUD игрока — да; HUD целиком — нет.
  if (process.platform === "win32")
    assert.deepEqual((await tool.inspect(done.id)).render.view, { keyhud: true, hud: false, ownConfig: true, ownHud: true });
  assert.equal(quit.incomplete, true);
  assert.deepEqual(pick((await tool.inspect(quit.id)).markers).filter((marker) => marker[1] === "finish"), [], "ран не закончен — финиша нет");
  // В видео (jobs.js): поверх меток адаптера, вырезанное и скорость учтены, тип сохраняется.
  const extra = await tool.runMarkers(tool.resolve(done.id));
  const planner = require("../src/main/demo-tool/planner");
  const mapped = planner.mapTimeline([{ t: 0, type: "map", label: "d1_trainstation_05" }].concat(extra),
    { from: 1, to: 6, speed: 2, offset: 1, cuts: [{ from: 2, to: 2.5 }] });
  assert.deepEqual(mapped.map((marker) => [marker.t, marker.type, marker.label]), [[0.75, "chapter", "13"], [1.5, "finish", "0:04.750"]]);

  const hl1 = path.join(dir, "a.dem");
  fs.writeFileSync(hl1, fx.goldsrcDemo());
  tool.addPaths([hl1]);
  const single = tool.list().items.find((item) => item.engine === "goldsrc");
  assert.deepEqual(await tool.runMarkers(tool.resolve(single.id)), []);
});

test("«Отрендеренные»: только видео со своим timeline.json, имена без путей, открытие и удаление в Корзину", async (t) => {
  const dir = tempDir(t);
  const videos = path.join(dir, "Videos");
  const out = path.join(videos, "HLSR");
  const trash = path.join(dir, "trash");
  fs.mkdirSync(out, { recursive: true });
  fs.mkdirSync(trash);
  let trashFails = false;
  const shell = {
    trashItem: async (file) => {
      if (trashFails) throw new Error("busy");
      fs.renameSync(file, path.join(trash, path.basename(file)));
    },
  };
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: path.join(dir, "jobs"), production: true, videosDir: videos, shell });
  t.after(() => tool.dispose());
  const timeline = (extra) => JSON.stringify(Object.assign({
    version: 1,
    source: { files: ["run.ghplaylist", "segment001.dem"], engine: "source", game: "ghosting", map: "d1_trainstation_01" },
    video: { file: "x.mp4", duration: 12.5, fps: 60, width: 1920, height: 1080, speed: 1, range: { from: 0, to: 12.5 }, encoder: "libx264" },
    markers: [{ t: 0, type: "map", label: "d1_trainstation_01", sourceTime: 0 }, { t: 3, type: "finish", label: "1:23:45.678", sourceTime: 3 },
      { t: 4, type: "<script>", label: "x" }, { t: "5", type: "timer", label: "1:00.000" }],
  }, extra));
  fs.writeFileSync(path.join(out, "run.mp4"), "video");
  fs.writeFileSync(path.join(out, "run.timeline.json"), timeline());
  fs.writeFileSync(path.join(out, "older.mp4"), "video");
  fs.writeFileSync(path.join(out, "older.timeline.json"), timeline());
  fs.utimesSync(path.join(out, "older.mp4"), new Date(2020, 0, 1), new Date(2020, 0, 1));
  fs.writeFileSync(path.join(out, "holiday.mp4"), "чужое видео без таймлайна");
  fs.writeFileSync(path.join(out, "broken.mp4"), "video");
  fs.writeFileSync(path.join(out, "broken.timeline.json"), "{");

  const list = tool.renderedList();
  assert.equal(list.outputDir, out);
  assert.deepEqual(list.items.map((item) => item.name), ["run.mp4", "older.mp4"]);
  assert.deepEqual(list.items[0].source, { game: "ghosting", map: "d1_trainstation_01", files: 2 });
  assert.equal(list.items[0].video.duration, 12.5);

  const opened = tool.renderedOpen("run.mp4");
  assert.equal(opened.status, "ok");
  assert.match(opened.url, /^hlsr-media:\/\/media\/[0-9a-f]{32}$/);
  assert.deepEqual(opened.markers.map((marker) => [marker.t, marker.type]), [[0, "map"], [3, "finish"]], "неизвестные типы и t не числом отброшены");
  assert.equal((await tool.media.handle(new Request(opened.url))).status, 200);
  const second = tool.renderedOpen("older.mp4");
  assert.equal((await tool.media.handle(new Request(opened.url))).status, 404, "у вкладки один адрес");

  for (const name of ["../run.mp4", "..\\run.mp4", "sub/run.mp4", "run.mp4:x.mp4", "C:\\Videos\\HLSR\\run.mp4", "run.timeline.json", "", null, 5])
    assert.throws(() => tool.renderedOpen(name), { code: "PATH_INVALID" }, String(name));
  assert.throws(() => tool.renderedOpen("missing.mp4"), { code: "RESULT_NOT_FOUND" });
  assert.throws(() => tool.renderedOpen("holiday.mp4"), { code: "RESULT_NOT_FOUND" });
  assert.throws(() => tool.renderedOpen("broken.mp4"), { code: "TIMELINE_INVALID" });

  trashFails = true;
  await assert.rejects(tool.renderedDelete("run.mp4"), { code: "TRASH_FAILED" });
  assert.ok(fs.existsSync(path.join(out, "run.mp4")));
  trashFails = false;
  const after = await tool.renderedDelete("older.mp4");
  assert.deepEqual(after.items.map((item) => item.name), ["run.mp4"]);
  assert.deepEqual(fs.readdirSync(trash).sort(), ["older.mp4", "older.timeline.json"]);
  assert.equal((await tool.media.handle(new Request(second.url))).status, 404, "удалённое видео больше не отдаётся");
  await assert.rejects(tool.renderedDelete("older.mp4"), { code: "RESULT_NOT_FOUND" });
  await assert.rejects(tool.renderedDelete("../older.mp4"), { code: "PATH_INVALID" });
  const noShell = new DemoTool({ store: fx.memoryStore(), jobsRoot: path.join(dir, "jobs2"), production: true, videosDir: videos });
  t.after(() => noShell.dispose());
  await assert.rejects(noShell.renderedDelete("run.mp4"), { code: "TRASH_FAILED" });
  assert.deepEqual(new DemoTool({ store: fx.memoryStore(), production: true, videosDir: path.join(dir, "none") }).renderedList().items, []);
});

test("предпросмотр: KeyHUD, HUD, конфиг и HUD игрока по умолчанию включены, выбор запоминается; FFmpeg — источники по порядку манифеста", async (t) => {
  const dir = tempDir(t);
  const store = fx.memoryStore();
  const tool = new DemoTool({ store, jobsRoot: path.join(dir, "jobs"), production: true, platform: "win32" });
  t.after(() => tool.dispose());
  assert.deepEqual(tool.settings().preview, { keyhud: true, hud: true, ownConfig: true, ownHud: true });
  await assert.rejects(tool.startPreview("missing", { keyhud: false, ownConfig: false, hud: "x" }), { code: "DEMO_NOT_FOUND" });
  assert.deepEqual(store.data.preview, { keyhud: false, hud: true, ownConfig: false, ownHud: true });
  assert.deepEqual(tool.settings().preview, { keyhud: false, hud: true, ownConfig: false, ownHud: true });
  await assert.rejects(tool.startPreview("missing", {}), { code: "DEMO_NOT_FOUND" });
  assert.deepEqual(store.data.preview, { keyhud: false, hud: true, ownConfig: false, ownHud: true }, "без выбора в окне — прежний");
  const status = await tool.ffmpegStatus();
  assert.deepEqual(status.origins, [{ site: "github.com/GyanD/codexffmpeg", release: "9.0.2" }, { site: "www.gyan.dev", release: null }]);
  assert.equal(status.sizeBytes, 114768076);
});

test("серии HL1 в папке игры: <имя>_<N> — один набор, числовой порядок, пропуски; одиночные и без номера — как были", () => {
  const demo = (name, dir) => ({ dir: dir || "Half-Life/valve_WON", name, size: 1, mtimeMs: 1 });
  const { series, rest } = library.groupSeries([
    demo("run_10.dem"), demo("run_2.dem"), demo("Run_1.dem"), demo("run_5.dem"),
    demo("solo_1.dem"), demo("plain.dem"), demo("other_x.dem"), demo("run_3.dem", "Half-Life/valve_WON_russian"),
    demo("zero_0.dem"), demo("zero_2.dem"),
  ]);
  assert.deepEqual(series.map((run) => [run.name, run.demos.map((item) => item.name), run.missing]), [
    ["run", ["Run_1.dem", "run_2.dem", "run_5.dem", "run_10.dem"], [3, 4, 6, 7, 8, 9]],
    ["zero", ["zero_0.dem", "zero_2.dem"], [1]],
  ]);
  assert.deepEqual(rest.map((item) => item.name).sort(), ["other_x.dem", "plain.dem", "run_3.dem", "solo_1.dem"]);
  assert.ok(rest.every((item) => item.number === undefined), "одиночные — без служебного номера");
});

test("DemoTool: серия HL1 из папки игры — один пункт с файлами по номерам, рендер всех подряд; Ghosting не группируется", async (t) => {
  const dir = tempDir(t);
  const hl1 = path.join(dir, "hl1");
  const mod = path.join(hl1, "Half-Life", "valve_WON");
  fs.mkdirSync(mod, { recursive: true });
  for (const [name, map] of [["hlsrrun_1", "c1a0"], ["hlsrrun_2", "c1a1"], ["hlsrrun_10", "c2a1"], ["hlsrrun_4", "c1a2"], ["myrun_1", "c1a0"], ["practice", "c1a0"]])
    fs.writeFileSync(path.join(mod, `${name}.dem`), fx.goldsrcDemo({ map }));
  const ghosting = path.join(dir, "gh");
  fs.mkdirSync(path.join(ghosting, "ghostingmod", "demos"), { recursive: true });
  for (const name of ["take_1.dem", "take_2.dem"]) fs.writeFileSync(path.join(ghosting, "ghostingmod", "demos", name), fx.sourceDemo());
  const tool = new DemoTool({
    store: fx.memoryStore(),
    games: () => [
      { gameId: "hl1-won", installPath: hl1, config: localConfigOf("hl1-won") },
      { gameId: "ghosting-renewed", installPath: ghosting, config: localConfigOf("ghosting-renewed") },
    ],
    jobsRoot: path.join(dir, "jobs"),
    production: true,
  });
  t.after(() => tool.dispose());
  const items = tool.list().items;
  const hl1Items = items.filter((item) => item.gameId === "hl1-won");
  assert.deepEqual(hl1Items.map((item) => `${item.kind}:${item.name}:${item.segmentCount}`).sort(),
    ["demo:myrun_1.dem:1", "demo:practice.dem:1", "segments:hlsrrun:4"], "файлы серии не дублируются");
  const series = hl1Items.find((item) => item.series);
  assert.deepEqual(series.missing, [3, 5, 6, 7, 8, 9]);
  assert.deepEqual(series.maps, ["c1a0", "c1a1", "c1a2", "c2a1"]);
  assert.equal(series.supported, true);
  assert.equal(series.duration, 4 * 12.75);
  const inspected = await tool.inspect(series.id);
  assert.deepEqual(inspected.segments.map((segment) => segment.name), ["hlsrrun_1.dem", "hlsrrun_2.dem", "hlsrrun_4.dem", "hlsrrun_10.dem"]);
  // Рендер — тем же путём, что ручной набор: все сегменты подряд, один материал.
  const goldsrcRender = require("../src/main/demo-tool/goldsrc-render");
  assert.deepEqual(goldsrcRender.planSegments(tool.resolve(series.id).segments, null).picked.map((entry) => entry.segment.name),
    ["hlsrrun_1.dem", "hlsrrun_2.dem", "hlsrrun_4.dem", "hlsrrun_10.dem"]);
  await assert.rejects(() => tool.reorder(series.id, [1, 0, 2, 3]), { code: "DEMO_NOT_FOUND" }, "порядок серии — по номерам");
  // Новая демка серии (ран ещё пишется): пункт тот же, выбор не теряется.
  fs.writeFileSync(path.join(mod, "hlsrrun_11.dem"), fx.goldsrcDemo({ map: "c2a2" }));
  const grown = tool.list().items.find((item) => item.series);
  assert.equal(grown.id, series.id);
  assert.equal(grown.segmentCount, 5);
  // Ghosting: свои плейлисты, <имя>_<N>.dem остаются отдельными.
  assert.deepEqual(items.filter((item) => item.gameId === "ghosting-renewed").map((item) => item.name).sort(), ["take_1.dem", "take_2.dem"]);
});

test("оценка времени: по умолчанию — по замеру стенда (Ghosting -ghrender 720p30), масштаб по разрешению, HL1 — запуск и демки", () => {
  const est = require("../src/main/demo-tool/estimate");
  const preview = (seconds, extra) => est.estimate(Object.assign({ profile: "ghosting-ghrender", launches: 1, segments: 60, seconds, fps: 30, width: 1280, height: 720,
    outputSeconds: seconds, preset: "preview", gpu: false }, extra), {});
  // Замер: ран 0:08 — 17–19 с, 2:24 — 103 с; ран 30:00 — около 21 мин.
  assert.ok(Math.abs(preview(8.16).seconds - 18) <= 3, String(preview(8.16).seconds));
  assert.ok(Math.abs(preview(144.46).seconds - 103) <= 15, String(preview(144.46).seconds));
  const long = preview(1800);
  assert.ok(long.seconds >= 20 * 60 && long.seconds <= 23 * 60, String(long.seconds));
  assert.deepEqual(Object.keys(long.parts), ["load", "render", "encode"]);
  assert.equal(long.learned, false);
  // Порог окна: короткие раны — без окна, получасовой — с ним.
  assert.ok(preview(144.46).seconds <= est.PREVIEW_PROMPT_SECONDS);
  assert.ok(long.seconds > est.PREVIEW_PROMPT_SECONDS);
  // 1080p60 «Лучшее качество» на процессоре: игра медленнее (пиксели), кадров вдвое больше, кодирование дольше.
  const quality = preview(1800, { fps: 60, width: 1920, height: 1080, preset: "quality" });
  assert.ok(quality.parts.render > long.parts.render * 2 && quality.parts.encode > long.parts.encode * 4);
  assert.ok(preview(600, { preset: "quality", gpu: true }).parts.encode < preview(600, { preset: "quality", gpu: false }).parts.encode);
  // Старый путь: запуск игры на каждую пачку; HL1 — запуск и подгрузка каждой демки набора.
  const batches = est.estimate({ profile: "ghosting-startdemos", launches: 3, segments: 50, seconds: 600, fps: 30, width: 1280, height: 720, outputSeconds: 600, preset: "preview" }, {});
  assert.equal(batches.parts.load, 36);
  const hl1 = est.estimate({ profile: "hl1", launches: 1, segments: 60, seconds: 1800, fps: 30, width: 1280, height: 720, outputSeconds: 1800, preset: "preview" }, {});
  assert.equal(hl1.parts.load, 10 + 59);
  // Неизвестный профиль — осторожная оценка, не ноль.
  assert.ok(est.estimate({ profile: "x", seconds: 60, fps: 30, width: 1280, height: 720, outputSeconds: 60, preset: "preview" }, {}).seconds > 40);
});

test("оценка времени: скорость прошлых заданий запоминается и заменяет замер стенда; короткие задания не в счёт", () => {
  const est = require("../src/main/demo-tool/estimate");
  // Задание: 4291 кадр за 108 с рендера (12 с — запуск по умолчанию), итог 4320 кадров кодировался 6 с.
  const stats = { profile: "ghosting-ghrender", launches: 1, segments: 60, frames: 4291, renderSeconds: 108, height: 720,
    preset: "preview", gpu: true, outputFrames: 4320, encodeSeconds: 6 };
  let learned = est.learn({}, stats);
  assert.deepEqual(learned.render["ghosting-ghrender@720"], { rate: 44.7, frames: 4291 });
  assert.deepEqual(learned.encode["preview@720"], { rate: 720, frames: 4320 });
  // Вдвое быстрее машина: следующее задание сдвигает среднее, вес прошлых ограничен.
  learned = est.learn(learned, Object.assign({}, stats, { renderSeconds: 12 + 4291 / 90 }));
  assert.ok(learned.render["ghosting-ghrender@720"].rate > 60 && learned.render["ghosting-ghrender@720"].rate < 70);
  const input = { profile: "ghosting-ghrender", launches: 1, seconds: 1800, fps: 30, width: 1280, height: 720, outputSeconds: 1800, preset: "preview" };
  const own = est.estimate(input, learned);
  assert.equal(own.learned, true);
  assert.ok(own.seconds < est.estimate(input, {}).seconds);
  // Другое разрешение того же пути — пересчёт от своего замера, а не от стенда.
  const at1080 = est.estimate(Object.assign({}, input, { width: 1920, height: 1080 }), learned);
  assert.ok(at1080.parts.render > own.parts.render && at1080.parts.render < est.estimate(Object.assign({}, input, { width: 1920, height: 1080 }), {}).parts.render);
  // Короткий ран (почти весь — запуск игры) и задание без профиля скорость не трогают.
  assert.deepEqual(est.learn({}, Object.assign({}, stats, { frames: 245, outputFrames: 245 })), { render: {}, encode: {} });
  assert.deepEqual(est.learn({}, Object.assign({}, stats, { profile: null, outputFrames: 0 })), { render: {}, encode: {} });
  // Discord: свой ключ кодирования по процессору/GPU.
  assert.equal(est.encodeKind("discord", false), "discord-cpu");
  assert.equal(est.encodeKind("quality", true), "quality-gpu");
  assert.equal(est.encodeKind("preview", false), "preview");
});

test("DemoTool: оценка по интервалу и пресетам; готовое задание запоминает скорость в store инструмента", async (t) => {
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  fx.writeGhostingRun(path.join(install, "ghostingmod", "demos"), "long", [{ map: "m1", duration: 600 }, { map: "m2", duration: 1200 }]);
  const store = fx.memoryStore();
  const tool = new DemoTool({
    store,
    games: () => [{ gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") }],
    jobsRoot: path.join(dir, "jobs"),
    production: true,
    platform: "win32",
  });
  t.after(() => tool.dispose());
  const id = tool.list().items[0].id;
  const answer = await tool.estimate(id, [{ preset: "preview" }, { preset: "preview", range: { from: 60, to: 360 } }, { preset: "quality", height: 1080, fps: 60 }]);
  assert.equal(answer.status, "ok");
  assert.equal(answer.threshold, 120);
  const [whole, part, quality] = answer.items;
  // Без мода с -ghrender (нет client.dll) — старый путь: две демки одной пачкой.
  assert.deepEqual([whole.range, whole.segments, whole.parts.load], [1800, 2, 12]);
  assert.ok(whole.seconds > answer.threshold);
  assert.equal(part.range, 300, "интервал «Начало/Конец здесь» — оценка по нему");
  assert.ok(part.seconds < whole.seconds / 4);
  assert.ok(quality.seconds > whole.seconds);
  await assert.rejects(tool.estimate(id, []), { code: "SETTINGS_INVALID" });
  await assert.rejects(tool.estimate("missing", [{ preset: "preview" }]), { code: "DEMO_NOT_FOUND" });
  // Готовое задание с замером: скорость в store, следующая оценка — по ней.
  const file = path.join(dir, "out.mp4");
  fs.writeFileSync(file, "video");
  tool.finished("job1", "export", { file, size: 5, attempts: 1, encoder: "libx264", warnings: [], timeline: { video: {}, markers: [] },
    stats: { profile: "ghosting-startdemos", launches: 1, segments: 2, frames: 18000, renderSeconds: 12 + 100, height: 720, preset: "preview",
      gpu: false, outputFrames: 18000, encodeSeconds: 20 } });
  assert.equal(store.data.speed.render["ghosting-startdemos@720"].rate, 180);
  const faster = (await tool.estimate(id, [{ preset: "preview" }])).items[0];
  assert.equal(faster.learned, true);
  assert.ok(faster.seconds < whole.seconds / 2);
});

test("«Отрендеренные»: исходная демка видео — по id из timeline.json, у старых видео по файлам, игре и карте, если такая одна", (t) => {
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  const demos = path.join(install, "ghostingmod", "demos");
  fx.writeGhostingRun(demos, "a", [{ map: "d1_trainstation_01", duration: 2 }, { map: "d1_trainstation_01", duration: 1 }]);
  fx.writeGhostingRun(demos, "b", [{ map: "d1_trainstation_01", duration: 3 }, { map: "d1_trainstation_02", duration: 1 }]);
  fx.writeGhostingRun(demos, "c", [{ map: "d1_canals_01", duration: 3 }]);
  const tool = new DemoTool({
    store: fx.memoryStore(),
    games: () => [{ gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") }],
    jobsRoot: path.join(dir, "jobs"),
    production: true,
  });
  t.after(() => tool.dispose());
  const byName = (name) => tool.list().items.find((item) => item.name === name).id;
  const files2 = ["run.ghplaylist", "segment001.dem", "segment002.dem"];
  // Новые видео: id записи.
  assert.equal(tool.renderedSourceId({ source: { id: byName("b"), files: files2 } }), byName("b"));
  // Старые (без id): «a» и «b» с одинаковыми именами файлов и картой — неоднозначно, кнопка неактивна.
  assert.equal(tool.renderedSourceId({ source: { files: files2, game: "ghosting", map: "d1_trainstation_01" } }), null);
  // Одна подходящая — она.
  assert.equal(tool.renderedSourceId({ source: { files: ["run.ghplaylist", "segment001.dem"], game: "ghosting", map: "d1_canals_01" } }), byName("c"));
  // Исходной демки больше нет: id неизвестен, по файлам ничего.
  assert.equal(tool.renderedSourceId({ source: { id: "0123456789abcdef", files: ["gone.dem"], game: "hl1", map: "c1a0" } }), null);
  assert.equal(tool.renderedSourceId({}), null);
  assert.equal(tool.renderedSourceId(null), null);
});
