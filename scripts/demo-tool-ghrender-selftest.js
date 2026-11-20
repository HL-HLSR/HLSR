"use strict";

// «Демо»: рендер Ghosting в режиме мода -ghrender (протокол журнала GHRENDER 2) —
// выбор пути по client.dll и папке рана, имя плейлиста, диапазон записей, разбор
// журнала, коды ошибок, таймлайн и вырезанное из номеров кадров, откат на старый
// путь, сброс mute без рендера, адаптер с подставной игрой.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { EventEmitter } = require("node:events");
const { Writable, Readable } = require("node:stream");

const gh = require("../src/main/demo-tool/ghrender");
const ghosting = require("../src/main/demo-tool/ghosting");
const planner = require("../src/main/demo-tool/planner");
const library = require("../src/main/demo-tool/library");
const sr = require("../src/main/demo-tool/source-render");
const fx = require("./lib/demo-fixtures");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ghr-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));
  return dir;
}

// client.dll мода: строки параметров -ghrender (без них — мод без режима).
function writeClient(modDir, strings) {
  fs.mkdirSync(path.join(modDir, "bin"), { recursive: true });
  fs.writeFileSync(path.join(modDir, "bin", "client.dll"), Buffer.concat([Buffer.alloc(4096, 0x90), Buffer.from(strings.join("\0"), "latin1"), Buffer.alloc(16)]));
}

test("умеет ли мод -ghrender — по строкам в client.dll, не по номеру версии; кэш по размеру и mtime", (t) => {
  const mod = path.join(tempDir(t), "ghostingmod");
  assert.deepEqual(gh.clientCaps(mod), { ghrender: false, segments: false }, "нет client.dll");
  writeClient(mod, ["-ghrender", "-ghrender_out", "-ghrender_fps"]);
  assert.deepEqual(gh.clientCaps(mod), { ghrender: true, segments: false });
  writeClient(mod, ["-ghrender", "-ghrender_out", "-ghrender_segments", "-ghrender_fps"]);
  assert.deepEqual(gh.clientCaps(mod), { ghrender: true, segments: true }, "новый файл — новый ответ");
  writeClient(mod, ["-game", "+exec"]);
  assert.deepEqual(gh.clientCaps(mod), { ghrender: false, segments: false }, "мод 1.0.12");
  // Замена файлом того же размера с тем же mtime (так копируют robocopy и 7z) — кэш её видит.
  const file = path.join(mod, "bin", "client.dll");
  writeClient(mod, ["-ghrender_out", "-ghrender_segments"]);
  const stamp = new Date(Date.now() - 3600 * 1000);
  fs.utimesSync(file, stamp, stamp);
  assert.deepEqual(gh.clientCaps(mod), { ghrender: true, segments: true });
  writeClient(mod, ["-ghrender_xyz", "-ghrender_abcdefgh"]);
  fs.utimesSync(file, stamp, stamp);
  assert.deepEqual(gh.clientCaps(mod), { ghrender: false, segments: false }, "тот же размер и mtime, другое содержимое");
  // Больше 64 МиБ — не client.dll мода: не читается.
  fs.truncateSync(file, gh.MAX_CLIENT_BYTES + 1);
  fs.appendFileSync(file, "-ghrender_out -ghrender_segments");
  assert.deepEqual(gh.clientCaps(mod), { ghrender: false, segments: false });
});

test("имя рана для -ghrender: только формы мода внутри demos/, без чужих символов", () => {
  const mod = path.join("C:", "Games", "Ghosting", "ghostingmod");
  const at = (...parts) => path.join(mod, ...parts);
  assert.equal(gh.playlistArg(mod, at("demos", "myrun2", "run.ghplaylist")), "demos/myrun2/run.ghplaylist");
  assert.equal(gh.playlistArg(mod, at("demos", "gh_latest.ghplaylist")), "gh_latest.ghplaylist");
  assert.equal(gh.playlistArg(mod, at("gh_latest.ghplaylist")), null, "вне demos/ мод не ищет");
  assert.equal(gh.playlistArg(mod, at("demos", "my run", "run.ghplaylist")), null, "пробел");
  assert.equal(gh.playlistArg(mod, at("demos", "-ghrender_out", "run.ghplaylist")), null, "ведущий «-»");
  assert.equal(gh.playlistArg(mod, at("demos", "+exec", "run.ghplaylist")), null);
  assert.equal(gh.playlistArg(mod, at("demos", "a", "b", "run.ghplaylist")), null, "вложенность");
  assert.equal(gh.playlistArg(mod, at("demos", "myrun2", "other.ghplaylist")), null);
  assert.equal(gh.playlistArg(mod, at("demos", "мой", "run.ghplaylist")), null, "не ASCII");
  assert.equal(gh.playlistArg(mod, path.join("D:", "elsewhere", "demos", "x", "run.ghplaylist")), null);
  const args = gh.gameArguments({ width: 1920, height: 1080, fps: 60, keyhud: true, integrity: false, playlist: "demos/myrun2/run.ghplaylist", out: "hlsr-render/ab12cd34" });
  const value = (name) => args[args.indexOf(name) + 1];
  assert.deepEqual([value("-ghrender"), value("-ghrender_out"), value("-ghrender_fps"), value("-ghrender_keyshud"), value("-ghrender_intact"), value("+volume")],
    ["demos/myrun2/run.ghplaylist", "hlsr-render/ab12cd34", "60", "1", "0", "0.8"]);
  // Консоль рендера — в папке задания, не в con_logfile игрока.
  assert.equal(value("+con_logfile"), "hlsr-render/ab12cd34/console.log");
  assert.equal(args[args.indexOf("+demo_interplimit") + 1], "99999");
  assert.equal(args[args.indexOf("+demo_avellimit") + 1], "99999");
  for (const old of ["+startdemos", "+startmovie", "+host_framerate", "+snd_mixahead"]) assert.ok(!args.includes(old), old);
  const other = gh.gameArguments({ width: 1, height: 1, fps: 30, keyhud: false, integrity: true, playlist: "p", out: "o" });
  assert.deepEqual([other[other.indexOf("-ghrender_keyshud") + 1], other[other.indexOf("-ghrender_intact") + 1]], ["0", "1"]);
});

test("папка рана: -ghrender только если пути записей плейлиста ведут в его собственную папку", (t) => {
  const seg = (name, playlistFile) => ({ name, playlistFile });
  assert.equal(gh.entriesInPlace("demos/run1/run.ghplaylist", [seg("segment001.dem", "demos/run1/segment001.dem")]), true);
  assert.equal(gh.entriesInPlace("demos/Run1/run.ghplaylist", [seg("segment001.dem", "demos/run1/SEGMENT001.dem")]), true, "регистр не важен");
  assert.equal(gh.entriesInPlace("demos/best/run.ghplaylist", [seg("segment001.dem", "demos/run1/segment001.dem")]), false, "переименована");
  assert.equal(gh.entriesInPlace("gh_latest.ghplaylist", [seg("a.dem", "demos/a.dem")]), true, "плоский плейлист");
  assert.equal(gh.entriesInPlace("gh_latest.ghplaylist", [seg("a.dem", "demos/run1/a.dem")]), false);
  assert.equal(gh.entriesInPlace("demos/run1/run.ghplaylist", [seg("segment001.dem", undefined)]), false);
  assert.equal(gh.entriesInPlace("demos/run1/run.ghplaylist", []), false);
  // Проба ревьюера (probe-rename.js): ран из demos/run1 переименован в best.v2 и скопирован в friend3 — старый путь.
  const mod = path.join(tempDir(t), "game", "ghostingmod");
  const demos = path.join(mod, "demos");
  fx.writeGhostingRun(demos, "run1", [{ map: "m1", duration: 3 }, { map: "m1", duration: 2 }]);
  fs.cpSync(path.join(demos, "run1"), path.join(demos, "friend3"), { recursive: true });
  fs.renameSync(path.join(demos, "run1"), path.join(demos, "best.v2"));
  for (const name of ["best.v2", "friend3"]) {
    const item = library.describe({ kind: "playlist", files: [path.join(demos, name, "run.ghplaylist")], name });
    assert.equal(item.supported, true, name);
    assert.equal(item.segments[0].playlistFile, "demos/run1/segment001.dem");
    const arg = gh.playlistArg(mod, item.files[0]);
    assert.equal(arg === null || !gh.entriesInPlace(arg, item.segments), true, name);
  }
});

test("журнал: строки ключ=значение, FAIL -> коды лаунчера с номером сегмента", () => {
  assert.deepEqual(gh.parseLine("SEGMENT n=6 entry=14/66 frame=2294 kind=load map=d1_trainstation_02\r"),
    { key: "SEGMENT", fields: { n: "6", entry: "14/66", frame: "2294", kind: "load", map: "d1_trainstation_02" }, text: "SEGMENT n=6 entry=14/66 frame=2294 kind=load map=d1_trainstation_02" });
  assert.equal(gh.parseLine("   "), null);
  assert.equal(gh.parseLine("GHRENDER 2").text, gh.PROTOCOL);
  const fail = (line) => gh.failError(gh.parseLine(line));
  assert.deepEqual([fail("FAIL code=SEGMENT_CHANGED entry=11/14 file=demos/badseg/segment011.dem frames=84").code, fail("FAIL code=SEGMENT_CHANGED entry=11/14 frames=84").details],
    ["SEGMENT_CHANGED", { segment: 11 }]);
  assert.deepEqual(fail("FAIL code=SEGMENT_MISSING entry=13/14 frames=1539").details, { segment: 13 });
  const codes = { BAD_OPTION: "BAD_OPTION", PLAYLIST_INVALID: "PLAYLIST_INVALID", MAP_MISSING: "MAP_MISSING", SEGMENT_MALFORMED: "SEGMENT_MALFORMED",
    SEGMENT_UNSAFE: "SEGMENT_UNSAFE", DEMO_FAILED: "DEMO_OPEN_FAILED", WRITE_FAILED: "RENDER_FAILED", NO_FRAMES: "RENDER_FAILED", SOMETHING_NEW: "RENDER_FAILED" };
  for (const [from, to] of Object.entries(codes)) assert.equal(fail(`FAIL code=${from} frames=0`).code, to, from);
});

// Ран из 5 записей (время демо 0..10): записи 2 и 4 пустые (мод их пропускает),
// мод отбрасывает по 0,05 с в начале показанных сегментов (timer − start).
const RUN = { durations: [3, 0.01, 4, 0.01, 2.98], fps: 60 };
const STARTS = [0, 3, 3.01, 7.01, 7.02];
const JOURNAL = [
  "GHRENDER 2",
  "JOB playlist=demos/run1/run.ghplaylist out=hlsr-render/x fps=60 width=8 height=4 keyshud=1 intact=0",
  "SEGMENT n=1 entry=1/5 frame=0 kind=start map=m1 start=0.000 end=3.000 timer=0.050 file=demos/run1/segment001.dem",
  "TIMER frame=0 event=start timer=0.000",
  "SEGMENT n=2 entry=3/5 frame=177 kind=load map=m1 start=3.010 end=7.010 timer=3.060 file=demos/run1/segment003.dem",
  "PROGRESS segment=2 entry=3/5 frames=180",
  "SEGMENT n=3 entry=5/5 frame=414 kind=map map=m2 start=7.020 end=10.000 timer=7.070 file=demos/run1/segment005.dem",
  "TIMER frame=588 event=finish timer=9.950",
  "DONE frames=590 segments=3 fps=60 audio=audio.wav samples=433650 rate=44100 channels=2 blocks=3 data=intact",
];

test("командная строка -ghrender: стандартный конфиг и HUD — после остальных +команд", () => {
  const sr = require("../src/main/demo-tool/source-render");
  const base = { width: 1920, height: 1080, fps: 60, keyhud: true, integrity: false, playlist: "demos/run1/run.ghplaylist", out: "hlsr-render/ab12cd34" };
  const plain = gh.gameArguments(base);
  assert.deepEqual(gh.gameArguments(Object.assign({ config: [] }, base)), plain);
  assert.ok(!plain.includes("-insert_search_path"));
  const config = sr.configArguments({ ownConfig: false, ownHud: true, dir: "ghostingmod/hlsr-render/ab12cd34/std" }).args;
  const args = gh.gameArguments(Object.assign({ config }, base));
  assert.deepEqual(args.slice(0, plain.length), plain);
  assert.deepEqual(args.slice(plain.length), ["-insert_search_path", "ghostingmod/hlsr-render/ab12cd34/std", "+exec", "hlsr-personalization.cfg"]);
});

test("таймлайн из журнала: метки на кадрах SEGMENT, вырезанное, хвост; интервал режется общим слоем", () => {
  const seg = JOURNAL.map(gh.parseLine).filter((line) => line && line.key === "SEGMENT").map((line) => line.fields);
  const timers = JOURNAL.map(gh.parseLine).filter((line) => line && line.key === "TIMER").map((line) => line.fields);
  const result = gh.timelineOf({ segments: seg, timers, frames: 590, fps: RUN.fps, starts: STARTS, total: 10 });
  assert.equal(result.offset, 0.05);
  // Между сегментами: конец прошлого (по кадрам) -> начало следующего + отброшенное.
  assert.deepEqual(result.cuts, [{ from: 3, to: 3.06 }, { from: 7.01, to: 7.07 }, { from: 10.003, to: 10 }].filter((cut) => cut.to > cut.from));
  // Во времени видео (весь ран, 1×) метки ровно на frame/fps своих SEGMENT.
  const video = planner.mapTimeline(result.markers, { from: 0, to: 10, speed: 1, offset: result.offset, cuts: result.cuts });
  assert.deepEqual(video.filter((marker) => marker.type !== "timer").map((marker) => [marker.t, marker.type, marker.label]),
    [[0, "map", "m1"], [2.95, "saveload", "3"], [6.9, "map", "m2"]]);
  // TIMER finish пропущен: в записи мода таймер уже остановлен, финиш ставит source-demo.js по RUN_END.
  assert.deepEqual(video.filter((marker) => marker.type === "timer").map((marker) => [marker.t, marker.label]),
    [[0, "0:00.050"], [2.95, "0:03.060"], [6.9, "0:07.070"]]);
  // Длина клипа — длина видео (590 кадров = 9,833 с; -t обрежет по ней).
  assert.equal(planner.clipDuration({ from: 0, to: 10, speed: 1, offset: result.offset, cuts: result.cuts }), 9.83);
  // Интервал 5–8 с при 0,5×: материал с 4,95−0,06 … , итог (3 − 0,06) / 0,5.
  assert.equal(planner.clipDuration({ from: 5, to: 8, speed: 0.5, offset: result.offset, cuts: result.cuts }), 5.88);
  // Интервал внутри карты: метка карты в его начале, как у старого пути.
  const range = gh.withRangeStart(result.markers, { from: 5, to: 8 });
  const clip = planner.mapTimeline(range, { from: 5, to: 8, speed: 0.5, offset: result.offset, cuts: result.cuts });
  assert.deepEqual(clip.filter((marker) => marker.type === "map").map((marker) => [marker.t, marker.label]), [[0, "m1"], [4.02, "m2"]]);
  assert.equal(gh.withRangeStart(result.markers, null), result.markers);
  assert.equal(gh.withRangeStart(result.markers, { from: 7.07, to: 8 }).filter((marker) => marker.type === "map" && marker.t === 7.07).length, 1);
});

test("таймлайн: мод оставил больше кадров, чем длилась запись, — время демо не идёт назад", () => {
  const segments = [{ entry: "1/2", frame: "0", kind: "start", map: "m1", start: "0", timer: "0" }, { entry: "2/2", frame: "200", kind: "load", map: "m1", start: "3", timer: "3" }];
  const result = gh.timelineOf({ segments, timers: [], frames: 300, fps: 60, starts: [0, 3], total: 5 });
  // 200 кадров = 3,333 с > 3 с записи: второй сегмент начинается на 3,333, без отрицательного вырезания.
  assert.deepEqual(result.markers.filter((marker) => marker.type === "saveload").map((marker) => marker.t), [3.333]);
  assert.ok(result.cuts.every((cut) => cut.to > cut.from));
  assert.deepEqual(gh.timelineOf({ segments: [], timers: [], frames: 0, fps: 60, starts: [], total: 0 }), { offset: 0, cuts: [], markers: [] });
});

// ---------- адаптер: подставная игра с -ghrender ----------

const W = 8;
const H = 4;
function tga(width, height, tag) {
  const header = Buffer.alloc(18);
  header[2] = 2;
  header.writeUInt16LE(width, 12);
  header.writeUInt16LE(height, 14);
  header[16] = 24;
  const pixels = Buffer.alloc(width * height * 3);
  tag.copy(pixels);
  return Buffer.concat([header, pixels]);
}

function argv(line) {
  const out = [];
  for (const match of line.matchAll(/"((?:[^"\\]|\\.)*)"|(\S+)/g)) out.push(match[1] !== undefined ? match[1] : match[2]);
  return out;
}

// Как мод: журнал (первая строка — protocol), кадры f_NNNN.tga переименованием,
// audio.wav к DONE, выход через «1 с» после DONE/FAIL. Как помощник: глушение и сброс.
class FakeHost extends EventEmitter {
  constructor(game) {
    super();
    this.game = game;
    this.exitCode = null;
    this.closed = false;
    this.pending = null;
  }

  async start(request) {
    this.request = request;
    this.game.launches.push(request);
    if (request.resetOnly) {
      // Помощник в режиме resetOnly: один запуск, снять mute, «stopped» и выход.
      this.running = Promise.resolve();
      this.emit("event", { event: "started", pid: 0 });
      // stop() на нём только ждёт конца: сброс помощник делает сам.
      this.stopping = new Promise((resolve) => setImmediate(() => {
        const ok = !this.game.resetOnlyFails;
        this.emit("event", { event: "audio-reset", ok });
        this.pending = !ok;
        this.emit("event", { event: "stopped", code: 0, pending: !ok });
        this.closed = true;
        resolve();
      }));
      return 0;
    }
    this.running = this.simulate(argv(request.commandLine)).catch((error) => this.game.errors.push(error));
    this.emit("event", { event: "started", pid: 77 });
    this.emit("event", { event: "audio", action: "muted", pid: 77, device: 0, was: false });
    return 77;
  }

  async simulate(args) {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const value = (name) => args[args.indexOf(name) + 1];
    if (!args.includes("-ghrender")) {
      this.game.oldPath = true;
      while (!this.stopFlag) await sleep(10);
      return;
    }
    const out = path.join(this.game.modDir, ...value("-ghrender_out").split("/"));
    fs.mkdirSync(out, { recursive: true });
    const log = (line) => fs.appendFileSync(path.join(out, "ghrender.log"), `${line}\n`);
    // Мод не смог создать журнал: выход с кодом 2 без единой строки.
    if (this.game.exitSilently) {
      await sleep(20);
      this.exitCode = 2;
      this.emit("event", { event: "exited", code: 2 });
      return;
    }
    if (this.game.silent) {
      while (!this.stopFlag) await sleep(10);
      return;
    }
    await sleep(20);
    log(this.game.protocol || gh.PROTOCOL);
    this.game.segmentsArg = args.includes("-ghrender_segments") ? value("-ghrender_segments") : null;
    for (const line of this.game.journal) {
      if (this.stopFlag) return;
      const frames = /^SEGMENT .*frame=(\d+)/.exec(line);
      if (frames)
        while (this.game.frame < Number(frames[1])) this.writeFrame(out);
      if (/^(DONE|FAIL)/.test(line)) {
        if (/^DONE/.test(line)) {
          while (this.game.frame < this.game.totalFrames) this.writeFrame(out);
          fs.writeFileSync(path.join(out, "audio.wav"), sr.wavHeader({ channels: 2, rate: 44100, byteRate: 176400, blockAlign: 4, bits: 16 }, 0));
        }
        log(line);
        await sleep(30);
        this.exitCode = /^DONE/.test(line) ? 0 : 1;
        this.emit("event", { event: "exited", code: this.exitCode });
        return;
      }
      log(line);
      await sleep(5);
    }
    // Журнал кончился без DONE/FAIL: игра вышла сама или висит (застой).
    if (this.game.exitAfterJournal) {
      this.exitCode = 0;
      this.emit("event", { event: "exited", code: 0 });
    }
  }

  writeFrame(out) {
    const n = this.game.frame++;
    const tmp = path.join(out, `raw_${n}.tga`);
    fs.writeFileSync(tmp, tga(W, H, Buffer.from([n & 0xff, n >> 8])));
    fs.renameSync(tmp, path.join(out, `f_${String(n).padStart(4, "0")}.tga`));
  }

  get helperPid() {
    return null;
  }

  stop(reset) {
    if (this.stopping) return this.stopping;
    if (this.closed) return (this.stopping = Promise.resolve());
    this.stopFlag = true;
    this.game.resets.push(reset !== false);
    // resetFails: столько сбросов mute подряд не подтвердятся.
    const fail = reset !== false && this.game.resetFails > 0;
    if (fail) this.game.resetFails--;
    this.stopping = this.running.then(() => {
      if (reset !== false) this.emit("event", { event: "audio-reset", ok: !fail });
      this.pending = reset === false || fail;
      this.emit("event", { event: "stopped", code: 0, pending: this.pending });
    });
    return this.stopping;
  }

  detach() {
    this.stopFlag = true;
  }

  kill() {
    this.stopFlag = true;
  }
}

function fakeFfmpeg(sink) {
  return () => {
    const child = new EventEmitter();
    child.exitCode = null;
    child.stderr = Readable.from([]);
    child.stdin = new Writable({
      write(chunk, encoding, callback) {
        sink.frames.push(chunk[0] + chunk[1] * 256);
        callback();
      },
      final(callback) {
        callback();
        setImmediate(() => {
          child.exitCode = 0;
          child.emit("close", 0);
        });
      },
    });
    child.kill = () => {
      child.exitCode = 1;
      child.emit("close", 1);
    };
    return child;
  };
}

function setup(t, options) {
  const opts = options || {};
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  const modDir = path.join(install, "ghostingmod");
  fs.mkdirSync(modDir, { recursive: true });
  fs.writeFileSync(path.join(install, "hl2.exe"), "");
  // Номер версии на выбор пути не влияет: 1.0.12 с client.dll ветки -ghrender — режим мода.
  fs.writeFileSync(path.join(install, "installed.json"), JSON.stringify({ gameId: "ghosting-renewed", version: "1.0.12" }));
  // client: false — мод 1.0.12, "v1" — ветка с протоколом 1 (без диапазона), по умолчанию — протокол 2.
  writeClient(modDir, opts.client === false ? ["-game"] : opts.client === "v1" ? ["-ghrender", "-ghrender_out"] : ["-ghrender", "-ghrender_out", "-ghrender_segments"]);
  const playlist = fx.writeGhostingRun(path.join(modDir, "demos"), "run1", RUN.durations.map((duration, i) => ({ map: i < 4 ? "m1" : "m2", duration })));
  const item = library.describe({ kind: "playlist", files: [playlist], name: "run1" });
  const jobsRoot = path.join(dir, "jobs");
  const jobDir = path.join(jobsRoot, "abcdef0123456789");
  fs.mkdirSync(jobDir, { recursive: true });
  const keys = new Map([[ghosting.REGISTRY_KEY, { ScreenWidth: 2560 }]]);
  const reg = {
    exists: (key) => keys.has(key),
    exportKey: (key, file) => fs.writeFileSync(file, JSON.stringify(keys.get(key))),
    deleteKey: (key) => keys.delete(key),
    importFile: (file) => keys.set(ghosting.REGISTRY_KEY, JSON.parse(fs.readFileSync(file, "utf8"))),
  };
  const game = { modDir, journal: opts.journal || JOURNAL.slice(1), totalFrames: opts.totalFrames || 590, frame: 0, launches: [], errors: [], resets: [],
    protocol: opts.protocol, silent: opts.silent, exitAfterJournal: opts.exitAfterJournal, exitSilently: opts.exitSilently,
    resetFails: opts.resetFails || 0, resetOnlyFails: opts.resetOnlyFails };
  const sink = { frames: [] };
  const store = fx.memoryStore();
  const adapter = new ghosting.GhostingAdapter({
    platform: "win32", jobsRoot, store, registry: reg, script: "render-host.ps1", installPath: () => install,
    gameRunning: () => false, steamStatus: () => ({ ok: true }), inspector: { findByImageName: () => [], samePath: (a, b) => a === b },
    freeSpace: () => 1e15, hostFactory: () => new FakeHost(game), spawnFn: fakeFfmpeg(sink), ghrenderFirstLineMs: 300,
    // Застой -ghrender и старого пути (кадров нет) — коротко.
    ghrenderStallMs: opts.stallMs, frameTimeoutMs: opts.stallMs, firstFrameTimeoutMs: 300,
    ...(opts.adapter || {}),
  });
  const request = { item, range: Object.assign({ from: 0, to: item.duration }, opts.range || {}), fps: 60, width: W, height: H, audio: true, keyhud: true,
    integrity: false, jobDir, ffmpegPath: "ffmpeg", onProgress: () => {} };
  return { install, modDir, item, jobsRoot, jobDir, keys, game, sink, store, adapter, request };
}

test("адаптер, мод с -ghrender: весь ран одним запуском, без копий демок; кадры, звук и метки из журнала", { timeout: 60000 }, async (t) => {
  // Интервал 1 с – конец задевает все непустые сегменты: -ghrender.
  const { adapter, request, game, sink, keys, modDir, jobDir, jobsRoot, store } = setup(t, { range: { from: 1 } });
  const progress = [];
  request.onProgress = (info) => progress.push(info);
  // Что лежало в папке задания в моде по ходу рендера: копий демок там быть не должно.
  const seen = new Set();
  const job = path.join(modDir, "hlsr-render", "abcdef01");
  const timer = setInterval(() => { if (fs.existsSync(job)) for (const name of fs.readdirSync(job)) seen.add(name.replace(/\d+/g, "N")); }, 5);
  const material = await adapter.render(request);
  clearInterval(timer);
  assert.deepEqual(game.errors, []);
  assert.equal(game.launches.length, 1, "один запуск, без пачек");
  const args = argv(game.launches[0].commandLine);
  assert.equal(args[args.indexOf("-ghrender") + 1], "demos/run1/run.ghplaylist");
  assert.equal(args[args.indexOf("-ghrender_out") + 1], "hlsr-render/abcdef01");
  assert.equal(args[args.indexOf("-ghrender_keyshud") + 1], "1");
  assert.ok(!args.includes("+startdemos"));
  // Сброс mute — обычным запуском без -ghrender.
  assert.ok(!argv(game.launches[0].resetCommandLine).includes("-ghrender"));
  // Интервал с 1 с задевает все непустые записи: весь ран, без -ghrender_segments; режет общий слой.
  assert.equal(game.segmentsArg, null);
  assert.equal(sink.frames.length, 590);
  assert.deepEqual(sink.frames.slice(0, 3), [0, 1, 2]);
  assert.equal(material.video, path.join(jobDir, "video.mkv"));
  assert.equal(material.audio, path.join(jobDir, "audio.wav"));
  assert.ok(fs.existsSync(material.audio), "audio.wav перенесён в папку задания");
  assert.equal(material.offset, 0.05);
  assert.deepEqual(material.cuts, [{ from: 3, to: 3.06 }, { from: 7.01, to: 7.07 }]);
  assert.deepEqual(material.timeline.filter((marker) => marker.type !== "timer").map((marker) => [marker.t, marker.type, marker.label]),
    [[0.05, "map", "m1"], [1, "map", "m1"], [3.06, "saveload", "3"], [7.07, "map", "m2"]]);
  assert.ok(progress.some((info) => info.segment && info.segment.index === 3));
  // Без копий: в папке задания в моде только журнал, кадры и звук мода; всё возвращено.
  assert.ok(seen.has("ghrender.log"));
  assert.deepEqual([...seen].filter((name) => /\.dem$/i.test(name)), []);
  assert.equal(fs.existsSync(path.join(modDir, "hlsr-render")), false);
  assert.deepEqual(keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.deepEqual(fs.readdirSync(jobsRoot), ["abcdef0123456789"]);
  assert.deepEqual(game.resets, [true]);
  assert.notEqual(store.get(ghosting.UNMUTE_PENDING), true);
});

// Диапазон 3-5 (интервал 5–8 с): мод показывает записи 3 и 5, кадры с 0, entry= — номер в плейлисте.
const RANGE_JOURNAL = [
  "JOB playlist=demos/run1/run.ghplaylist out=hlsr-render/x fps=60 width=8 height=4 keyshud=1 intact=0 segments=3-5",
  "SEGMENT n=1 entry=3/5 frame=0 kind=start map=m1 start=3.010 end=7.010 timer=3.060 tick=5 file=demos/run1/segment003.dem",
  "SEGMENT n=2 entry=5/5 frame=237 kind=map map=m2 start=7.020 end=10.000 timer=7.070 tick=5 file=demos/run1/segment005.dem",
  "TIMER frame=411 event=finish timer=9.950",
  "DONE frames=413 segments=2 fps=60 audio=audio.wav samples=302848 rate=44100 channels=2 blocks=2 data=intact",
];

test("адаптер -ghrender: интервал — только задетые записи (-ghrender_segments), время и метки из журнала со сдвигом", { timeout: 60000 }, async (t) => {
  const s = setup(t, { range: { from: 5, to: 8 }, journal: RANGE_JOURNAL, totalFrames: 413 });
  const material = await s.adapter.render(s.request);
  assert.deepEqual(s.game.errors, []);
  assert.equal(s.game.launches.length, 1);
  assert.equal(s.game.segmentsArg, "3-5");
  assert.equal(s.sink.frames.length, 413);
  // Начало материала — запись 3 после отброшенного до таймера (3,01 + 0,05), вырезано — между записями.
  assert.equal(material.offset, 3.06);
  assert.deepEqual(material.cuts, [{ from: 7.01, to: 7.07 }]);
  // Тот же клип, что из всего рана (5,88 с на 0,5×), и метки на тех же местах.
  assert.equal(planner.clipDuration({ from: 5, to: 8, speed: 0.5, offset: material.offset, cuts: material.cuts }), 5.88);
  const clip = planner.mapTimeline(material.timeline, { from: 5, to: 8, speed: 0.5, offset: material.offset, cuts: material.cuts });
  assert.deepEqual(clip.filter((marker) => marker.type === "map").map((marker) => [marker.t, marker.label]), [[0, "m1"], [4.02, "m2"]]);
  assert.deepEqual(s.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(s.modDir, "hlsr-render")), false);
  // Номера записей: пустые тоже считаются; одна запись — «N»; все непустые — без параметра.
  const picked = (indexes) => indexes.map((index) => ({ index }));
  assert.equal(gh.segmentsArg(picked([2, 4]), 3), "3-5");
  assert.equal(gh.segmentsArg(picked([13]), 20), "14");
  assert.equal(gh.segmentsArg(picked([0, 2, 4]), 3), null);
  const args = gh.gameArguments({ width: 1, height: 1, fps: 60, keyhud: false, integrity: false, playlist: "p", out: "o", segments: "12-14" });
  assert.equal(args[args.indexOf("-ghrender_segments") + 1], "12-14");
});

test("адаптер: мод без -ghrender или с протоколом 1, ран не из своей папки — старый путь (startdemos)", { timeout: 60000 }, async (t) => {
  for (const client of [false, "v1"]) {
    const { adapter, request, game } = setup(t, { client, range: { from: 5, to: 8 } });
    await assert.rejects(adapter.render(request), { code: "RENDER_TIMEOUT" });
    assert.equal(game.oldPath, true);
    assert.equal(game.launches.length, 1);
    const args = argv(game.launches[0].commandLine);
    assert.ok(!args.includes("-ghrender"));
    assert.equal(args[args.indexOf("+startdemos") + 1].split(" ").length, 2, "только сегменты интервала");
  }
  // Ран не из demos/ мода (папка мода целиком), не плейлист, переименованная папка — тоже старый путь.
  const other = setup(t);
  assert.equal(other.adapter.ghrenderPlaylist(other.install, other.item), "demos/run1/run.ghplaylist");
  const outside = path.join(other.modDir, "run.ghplaylist");
  fs.copyFileSync(other.item.files[0], outside);
  assert.equal(other.adapter.ghrenderPlaylist(other.install, Object.assign({}, other.item, { files: [outside] })), null);
  assert.equal(other.adapter.ghrenderPlaylist(other.install, Object.assign({}, other.item, { kind: "demo" })), null);
  const demos = path.join(other.modDir, "demos");
  fs.renameSync(path.join(demos, "run1"), path.join(demos, "friend3"));
  const renamed = library.describe({ kind: "playlist", files: [path.join(demos, "friend3", "run.ghplaylist")], name: "friend3" });
  assert.equal(other.adapter.ghrenderPlaylist(other.install, renamed), null);
});

test("адаптер -ghrender: нет «GHRENDER 2» вовремя или другой протокол (и 1) — игра закрыта, тот же экспорт старым путём", { timeout: 60000 }, async (t) => {
  // exitSilently — мод вышел с кодом 2, не создав журнала; resetFails — сброс mute первой
  // попытки не подтвердился (флаг снимет подтверждённый сброс старого пути).
  for (const options of [{ silent: true }, { protocol: "GHRENDER 1" }, { protocol: "GHRENDER 9" }, { exitSilently: true, resetFails: 1 }]) {
    const s = setup(t, options);
    // Старый путь у подставной игры кадров не даёт: RENDER_TIMEOUT уже от него.
    await assert.rejects(s.adapter.render(s.request), { code: "RENDER_TIMEOUT" });
    assert.equal(s.game.launches.length, 2, "один откат");
    assert.ok(argv(s.game.launches[0].commandLine).includes("-ghrender"));
    assert.ok(argv(s.game.launches[1].commandLine).includes("+startdemos"));
    assert.equal(s.game.oldPath, true);
    assert.deepEqual(s.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
    assert.equal(fs.existsSync(path.join(s.modDir, "hlsr-render")), false);
    assert.equal(s.adapter.isActive(), false);
    assert.notEqual(s.store.get(ghosting.UNMUTE_PENDING), true, "mute снят сбросом старого пути");
  }
});

test("адаптер -ghrender: диапазон без строк SEGMENT — понятная ошибка, не клип не из того места", { timeout: 60000 }, async (t) => {
  const s = setup(t, { range: { from: 5, to: 8 }, journal: RANGE_JOURNAL.filter((line) => !line.startsWith("SEGMENT")), totalFrames: 413 });
  await assert.rejects(s.adapter.render(s.request), (error) => error.code === "RENDER_FAILED" && /SEGMENT/.test(error.message));
  assert.deepEqual(s.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(s.modDir, "hlsr-render")), false);
});

test("Ghosting: mute после рендера снимается без экспорта — когда можно, беззвучным запуском, реестр как был", { timeout: 60000 }, async (t) => {
  const s = setup(t);
  assert.equal(s.adapter.muteResetBlocker(), "no-flag");
  s.store.set(ghosting.UNMUTE_PENDING, true);
  assert.equal(s.adapter.muteResetBlocker(), null);
  assert.equal(await s.adapter.resetMute(), true);
  assert.equal(s.game.launches.length, 1);
  const request = s.game.launches[0];
  assert.equal(request.resetOnly, true);
  const args = argv(request.resetCommandLine);
  assert.ok(args.includes("+volume") && !args.includes("-ghrender") && !args.includes("+startdemos"), args.join(" "));
  assert.notEqual(s.store.get(ghosting.UNMUTE_PENDING), true);
  assert.deepEqual(s.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(s.modDir, "hlsr-render")), false);
  assert.equal(s.adapter.isActive(), false);
  // Сброс не подтвердился — флаг остаётся.
  const failed = setup(t, { resetOnlyFails: true });
  failed.store.set(ghosting.UNMUTE_PENDING, true);
  assert.equal(await failed.adapter.resetMute(), false);
  assert.equal(failed.store.get(ghosting.UNMUTE_PENDING), true);
});

test("адаптер -ghrender: игра вышла без DONE, застой, нет карты, звук не записан, мусор в журнале", { timeout: 60000 }, async (t) => {
  const partial = [JOURNAL[1], JOURNAL[2], JOURNAL[3]];
  const exited = setup(t, { journal: partial, exitAfterJournal: true });
  await assert.rejects(exited.adapter.render(exited.request), { code: "GAME_EXITED" });
  const stalled = setup(t, { journal: partial, stallMs: 300 });
  await assert.rejects(stalled.adapter.render(stalled.request), (error) => error.code === "RENDER_TIMEOUT" && error.details.framesTotal > 0);
  const map = setup(t, { journal: [JOURNAL[1], "FAIL code=MAP_MISSING map=d1_canals_01 frames=0"] });
  await assert.rejects(map.adapter.render(map.request), { code: "MAP_MISSING" });
  for (const s of [exited, stalled, map]) {
    assert.deepEqual(s.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
    assert.equal(fs.existsSync(path.join(s.modDir, "hlsr-render")), false);
  }
  // DONE без звука — материал без звука (jobs.js предупредит AUDIO_UNAVAILABLE); длинная строка, чужой номер сегмента и лишние SEGMENT — мимо.
  const journal = JOURNAL.slice(1, -1).concat([`WARN ${"x".repeat(10000)}`, JOURNAL[JOURNAL.length - 1].replace("audio=audio.wav", "audio=none")]);
  journal[1] = journal[1].replace("n=1", "n=999999");
  const quiet = setup(t, { journal });
  const progress = [];
  quiet.request.onProgress = (info) => progress.push(info);
  const material = await quiet.adapter.render(quiet.request);
  assert.equal(material.audio, null);
  assert.equal(quiet.sink.frames.length, 590);
  assert.ok(progress.every((info) => !info.segment || (info.segment.index >= 1 && info.segment.index <= info.segment.count)));
});

test("адаптер -ghrender: FAIL из журнала — коды лаунчера, всё возвращено", { timeout: 60000 }, async (t) => {
  const changed = setup(t, { journal: [JOURNAL[1], JOURNAL[2], "FAIL code=SEGMENT_CHANGED entry=3/5 file=demos/run1/segment003.dem frames=84"] });
  await assert.rejects(changed.adapter.render(changed.request), (error) => error.code === "SEGMENT_CHANGED" && error.details.segment === 3);
  assert.deepEqual(changed.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(changed.modDir, "hlsr-render")), false);
  const bad = setup(t, { journal: ["FAIL code=BAD_OPTION frames=0"] });
  await assert.rejects(bad.adapter.render(bad.request), { code: "BAD_OPTION" });
});

test("адаптер -ghrender: отмена посреди рендера — CANCELLED, игра закрыта, всё возвращено", { timeout: 60000 }, async (t) => {
  const { adapter, request, game, keys, modDir } = setup(t, { journal: [JOURNAL[1], JOURNAL[2], "PROGRESS segment=1 entry=1/5 frames=60"] });
  game.journal.push(...new Array(200).fill("PROGRESS segment=1 entry=1/5 frames=60"));
  const controller = new AbortController();
  request.signal = controller.signal;
  const pending = adapter.render(request);
  setTimeout(() => controller.abort(), 100);
  await assert.rejects(pending, { code: "CANCELLED" });
  assert.deepEqual(keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(modDir, "hlsr-render")), false);
  assert.equal(adapter.isActive(), false);
});
