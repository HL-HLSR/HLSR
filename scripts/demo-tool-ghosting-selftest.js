"use strict";

// «Демо»: рендер Ghosting без настоящей игры — чистая логика (сегменты,
// пачки, таймлайн, журнал консоли, WAV, TGA, командная строка), реестр на
// подставном reg, восстановление по маркеру, чистка папок в моде, адаптер
// целиком с подставными помощником (игра пишет кадры, журнал и WAV) и
// FFmpeg, отказ запуска игры во время рендера. Настоящая игра — только
// HLSR_DEMO_GHOSTING_GAME (папка копии игры), иначе тест пропускается.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { EventEmitter } = require("node:events");
const { Writable, Readable } = require("node:stream");

const sr = require("../src/main/demo-tool/source-render");
const ghosting = require("../src/main/demo-tool/ghosting");
const planner = require("../src/main/demo-tool/planner");
const library = require("../src/main/demo-tool/library");
const { childEnv } = require("../src/main/demo-tool/render-host");
const DemoTool = require("../src/main/demo-tool");
const fx = require("./lib/demo-fixtures");

function tempDir(t, prefix) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix || "hlsr-gh-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));
  return dir;
}

const seg = (ticks, extra) => Object.assign({ ticks, duration: ticks / 100, tickrate: ticks ? 100 : null, map: "m1" }, extra);

test("сегменты: пустые (≤1 тик) пропускаются, интервал режет по сегментам, вырезанное и кадры", () => {
  const segments = [seg(145), seg(0), seg(1), seg(10), seg(2430), seg(1, { map: "m2" }), seg(300, { map: "m2" })];
  const full = sr.planSegments(segments, null, 60);
  assert.deepEqual(full.picked.map((item) => item.index), [0, 3, 4, 6]);
  assert.deepEqual(full.picked.map((item) => item.frames), [87, 6, 1458, 180]);
  assert.equal(full.framesTotal, 1731);
  assert.equal(full.offset, 0);
  assert.deepEqual(full.cuts, [{ from: 1.45, to: 1.46 }, { from: 25.86, to: 25.87 }]);
  assert.equal(full.framesNeeded, null);

  const mid = sr.planSegments(segments, { from: 5, to: 10 }, 30);
  assert.deepEqual(mid.picked.map((item) => item.index), [4]);
  assert.equal(mid.offset, 1.56);
  assert.deepEqual(mid.cuts, []);
  // До конца интервала 8,44 с материала: 254 кадра + 2 запаса из 729.
  assert.equal(mid.framesNeeded, 256);

  const across = sr.planSegments(segments, { from: 1, to: 26 }, 60);
  assert.deepEqual(across.picked.map((item) => item.index), [0, 3, 4, 6]);
  assert.equal(across.framesNeeded, Math.ceil((26 - 0 - 0.02) * 60) + 2);
  assert.equal(sr.planSegments([seg(1), seg(0)], null, 60).picked.length, 0);
});

test("пачки: не больше 32 имён и строка startdemos короче 512 символов", () => {
  const name = "hlsr-render/7eba6913/s001";
  const limit = sr.batchLimit(name.length);
  assert.equal(limit, 18);
  assert.ok(`startdemos ${Array(limit).fill(name).join(" ")}`.length < 512);
  assert.equal(sr.batchLimit(3), 32);
  const groups = sr.batches(Array.from({ length: 40 }, (_, i) => i), limit);
  assert.deepEqual(groups.map((group) => group.length), [18, 18, 4]);
  assert.deepEqual(groups.flat(), Array.from({ length: 40 }, (_, i) => i));
  assert.deepEqual(sr.batches([1, 2, 3], 100).map((group) => group.length), [3]);
});

test("таймлайн из плейлиста: начало, смены карт, сейвлоуды, таймер; интервал и время видео", () => {
  const segments = [
    seg(145, { timerStart: 0.03, timerEnd: 1.48 }),
    seg(1, { timerStart: 1.5, timerEnd: 1.51 }),
    seg(10, { timerStart: 1.57, timerEnd: 1.67 }),
    seg(500, { map: "m2", timerStart: 2, timerEnd: 7 }),
  ];
  const plan = sr.planSegments(segments, null, 60);
  const markers = sr.timelineOf(plan.picked, null);
  assert.deepEqual(markers, [
    { t: 0, type: "map", label: "m1" },
    { t: 0, type: "timer", label: "0:00.030" },
    { t: 1.46, type: "saveload", label: "3" },
    { t: 1.46, type: "timer", label: "0:01.570" },
    { t: 1.56, type: "map", label: "m2" },
    { t: 1.56, type: "timer", label: "0:02.000" },
    { t: 6.56, type: "timer", label: "0:07.000" },
  ]);
  // Как это увидит timeline.json: время видео без вырезанного пустого сегмента.
  const video = planner.mapTimeline(markers, { from: 0, to: 6.56, speed: 1, offset: plan.offset, cuts: plan.cuts });
  assert.deepEqual(video.map((marker) => [marker.t, marker.type]), [[0, "map"], [0, "timer"], [1.45, "saveload"], [1.45, "timer"],
    [1.55, "map"], [1.55, "timer"], [6.55, "timer"]]);
  // Интервал с середины сегмента: метка карты на начале интервала, таймера нет — он внутри сегмента.
  const ranged = sr.planSegments(segments, { from: 3, to: 5 }, 60);
  assert.deepEqual(sr.timelineOf(ranged.picked, { from: 3, to: 5 }).slice(0, 1), [{ t: 3, type: "map", label: "m2" }]);
  assert.equal(sr.clockLabel(125.25), "2:05.250");
});

test("журнал консоли: начало сегмента и ошибки движка", () => {
  assert.deepEqual(sr.parseLogLine("Playing demo from hlsr-render/ab12cd34/s001.dem."), { kind: "playing", demo: "hlsr-render/ab12cd34/s001" });
  assert.deepEqual(sr.parseLogLine("Playing demo from HLSR-render\\AB12CD34\\s002.dem.\r"), { kind: "playing", demo: "hlsr-render/ab12cd34/s002" });
  // Шаблоны привязаны к началу строк движка (+con_timestamp 0): шум developer 1 не ловится.
  for (const noise of ["material cubemap missing for env_cubemap", "Lightmap missing on map d1_canals_01", "[GH demo data] segment 1: demos/x/missing_map.dem (d1_town_01)",
    "echo Playing demo from x", "Could not find table \"ServerMapCycle\""]) assert.equal(sr.parseLogLine(noise), null, noise);
  assert.equal(sr.parseLogLine("Host_Error: Map is missing").code, "MAP_MISSING");
  assert.equal(sr.parseLogLine("Couldn't spawn server maps/x.bsp").code, "MAP_MISSING");
  assert.equal(sr.parseLogLine("CDemoFile::Open: couldn't open file hlsr-render/x/s001.dem for reading.").code, "DEMO_OPEN_FAILED");
  assert.equal(sr.parseLogLine("ERROR: demo network protocol 23 outdated, engine version is 24").code, "DEMO_PROTOCOL_UNSUPPORTED");
  assert.equal(sr.parseLogLine("Missing map maps/d1_canals_01a.bsp,  disconnecting").code, "MAP_MISSING");
  assert.equal(sr.parseLogLine("Host_Error: something bad").code, "GAME_ERROR");
  assert.equal(sr.parseLogLine("Redownloading all lightmaps"), null);
});

function tga(width, height, fill, options) {
  const header = Buffer.alloc(18);
  header[2] = 2;
  header.writeUInt16LE(width, 12);
  header.writeUInt16LE(height, 14);
  header[16] = 24;
  header[17] = options && options.topDown ? 0x20 : 0;
  const pixels = Buffer.alloc(width * height * 3, 0);
  fill.copy(pixels, 0);
  return Buffer.concat([header, pixels]);
}

test("TGA игры: размер, формат, порядок строк, недописанный кадр", () => {
  const full = tga(4, 2, Buffer.from([1, 2, 3]));
  assert.deepEqual(sr.tgaInfo(full), { width: 4, height: 2, bpp: 24, pixFmt: "bgr24", topDown: false, offset: 18, bytes: 24, complete: true });
  assert.equal(sr.tgaInfo(full.subarray(0, 30)).complete, false);
  assert.equal(sr.tgaInfo(tga(4, 2, Buffer.alloc(0), { topDown: true })).topDown, true);
  const rle = Buffer.from(full);
  rle[2] = 10;
  assert.equal(sr.tgaInfo(rle), null);
});

function wavFile(file, seconds, rate) {
  const blocks = Math.round(seconds * rate);
  const data = Buffer.alloc(blocks * 4);
  for (let i = 0; i < blocks; i++) data.writeInt16LE((i % 1000) + 1, i * 4);
  const header = sr.wavHeader({ channels: 2, rate, byteRate: rate * 4, blockAlign: 4, bits: 16 }, 0);
  header.writeUInt32LE(0, 4);
  fs.writeFileSync(file, Buffer.concat([header, data, Buffer.from([7])]));
}

test("WAV после TerminateProcess чинится; звук пачек склеивается ровно под кадры", async (t) => {
  const dir = tempDir(t);
  const one = path.join(dir, "one.wav");
  wavFile(one, 2.5, 44100);
  const layout = sr.fixWavFile(one);
  assert.equal(layout.length, 110250 * 4);
  assert.equal(fs.statSync(one).size, 44 + 110250 * 4);
  const fixed = fs.readFileSync(one);
  assert.equal(fixed.readUInt32LE(4), 36 + 110250 * 4);
  assert.equal(fixed.readUInt32LE(40), 110250 * 4);

  // Пачка 1: 2 с кадров + 0,117 с опережения; пачка 2 короче кадров (добивается тишиной).
  const a = path.join(dir, "a.wav");
  const b = path.join(dir, "b.wav");
  wavFile(a, 2 + 0.117, 44100);
  wavFile(b, 0.9, 44100);
  const joiner = new sr.WavJoiner(path.join(dir, "out.wav"));
  assert.equal(await joiner.append(a, 120, 60), 0.117);
  assert.equal(await joiner.append(b, 60, 60, sr.mixerLead(60)), 0.117);
  joiner.close();
  const out = fs.readFileSync(path.join(dir, "out.wav"));
  const data = out.readUInt32LE(40);
  assert.equal(data, (88200 + 44100) * 4);
  assert.equal(out.length, 44 + data);
  // Первая выборка пачки 1 — после опережения: 0,117 с = 5160 блоков.
  assert.equal(out.readInt16LE(44), (5160 % 1000) + 1);
  // Хвост пачки 2 — тишина.
  assert.equal(out.readInt16LE(out.length - 4), 0);
});

// Разбор командной строки, как CommandLineToArgvW (для проверки quoteArg).
function argv(line) {
  const out = [];
  let i = 0;
  while (i < line.length) {
    while (line[i] === " ") i++;
    if (i >= line.length) break;
    let arg = "";
    let quoted = false;
    for (; i < line.length && (quoted || line[i] !== " "); i++) {
      if (line[i] === "\\") {
        let n = 0;
        while (line[i] === "\\") { n++; i++; }
        if (line[i] === "\"") {
          arg += "\\".repeat(Math.floor(n / 2));
          if (n % 2) arg += "\"";
          else quoted = !quoted;
        } else {
          arg += "\\".repeat(n);
          i--;
        }
      } else if (line[i] === "\"") quoted = !quoted;
      else arg += line[i];
    }
    out.push(arg);
  }
  return out;
}

test("командная строка игры: startdemos и startmovie одним значением, кавычки по правилам Windows", () => {
  const args = sr.gameArguments({ width: 1920, height: 1080, fps: 60, keyhud: true, audio: true, demos: ["hlsr-render/ab/s001", "hlsr-render/ab/s002"], movie: "hlsr-render/ab/f_", logFile: "hlsr-render/ab/console-1.log" });
  const value = (name) => args[args.indexOf(name) + 1];
  assert.equal(value("+startdemos"), "hlsr-render/ab/s001 hlsr-render/ab/s002");
  assert.equal(value("+startmovie"), "hlsr-render/ab/f_ tga wav");
  assert.equal(value("+host_framerate"), "60");
  assert.equal(value("+gh_keyshud"), "1");
  assert.equal(value("+volume"), "0.8");
  assert.equal(value("+snd_mixahead"), "0.1");
  assert.equal(value("+demo_avellimit"), "99999");
  assert.equal(value("+demo_interplimit"), "99999");
  assert.equal(value("+demo_legacy_rollback"), "0");
  for (const flag of ["-windowed", "-noborder", "-novid", "-multirun", "-steam"]) assert.ok(args.includes(flag), flag);
  assert.ok(!args.includes("-condebug"), "журнал игрока console.log не трогаем");
  // Надпись «GH DEMO DATA: INTACT»: по умолчанию выключена, по выбору игрока — включена.
  assert.equal(value("+gh_demo_overlay"), "0");
  const shown = sr.gameArguments({ width: 1280, height: 720, fps: 30, keyhud: false, integrity: true, audio: true, demos: ["x"], movie: "m", logFile: "l" });
  assert.equal(shown[shown.indexOf("+gh_demo_overlay") + 1], "1");
  assert.equal(value("+startmovie"), "hlsr-render/ab/f_ tga wav");
  assert.equal(sr.gameArguments({ width: 1280, height: 720, fps: 30, keyhud: false, audio: false, demos: ["x"], movie: "m", logFile: "l" }).slice(-1)[0], "m tga");
  const exe = "C:\\Program Files\\HLSR\\Games\\ghosting renewed\\hl2.exe";
  const tricky = ["a b", "q\"uote", "tail\\", "dir\\\" x", ""];
  assert.deepEqual(argv(sr.commandLine(exe, args.concat(tricky))), [exe].concat(args, tricky));
});

test("окружение игры и помощника: без моста Discord, Slint/winit, Electron, NODE_OPTIONS и Steam-переменных", () => {
  const env = childEnv({ PATH: "x", HLSR_PRESENCE_PIPE: "p", hlsr_presence_pipe: "p", SLINT_BACKEND: "s", WINIT_X11_SCALE_FACTOR: "1",
    ELECTRON_RUN_AS_NODE: "1", NODE_OPTIONS: "--x", SteamAppId: "243730", SystemRoot: "C:\\Windows" });
  assert.deepEqual(Object.keys(env).sort(), ["PATH", "SystemRoot"]);
});

function fakeRegistry(initial) {
  const keys = new Map(initial ? [[ghosting.REGISTRY_KEY, Object.assign({}, initial)]] : []);
  const calls = [];
  return {
    keys,
    calls,
    failImport: false,
    exists: (key) => keys.has(key),
    exportKey(key, file) {
      calls.push("export");
      fs.writeFileSync(file, JSON.stringify(keys.get(key)));
    },
    deleteKey(key) {
      calls.push("delete");
      keys.delete(key);
    },
    importFile(file) {
      calls.push("import");
      if (this.failImport) throw Object.assign(new Error("reg import"), { code: "REGISTRY_FAILED" });
      keys.set(ghosting.REGISTRY_KEY, JSON.parse(fs.readFileSync(file, "utf8")));
    },
  };
}

test("реестр игрока: снимок до рендера, возврат после; ключа не было — созданный удаляется", (t) => {
  const root = tempDir(t);
  const reg = fakeRegistry({ ScreenWidth: 2560, ScreenHeight: 1440, ScreenWindowed: 0 });
  ghosting.snapshotRegistry(reg, root, { modJobDir: "x" });
  assert.equal(ghosting.readMarker(root).existed, true);
  reg.keys.set(ghosting.REGISTRY_KEY, { ScreenWidth: 1920, ScreenHeight: 1080, ScreenWindowed: 1, Extra: 5 });
  ghosting.restoreRegistry(reg, root);
  assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560, ScreenHeight: 1440, ScreenWindowed: 0 });
  assert.deepEqual(fs.readdirSync(root), []);

  const empty = fakeRegistry(null);
  ghosting.snapshotRegistry(empty, root);
  empty.keys.set(ghosting.REGISTRY_KEY, { ScreenWidth: 1920 });
  ghosting.restoreRegistry(empty, root);
  assert.equal(empty.keys.has(ghosting.REGISTRY_KEY), false);
  assert.deepEqual(empty.calls, ["delete"]);
  assert.equal(ghosting.restoreRegistry(empty, root), null, "без маркера ничего не делается");

  // Возврат не удался — маркер остаётся до следующей попытки.
  const broken = fakeRegistry({ ScreenWidth: 800 });
  ghosting.snapshotRegistry(broken, root);
  broken.failImport = true;
  assert.throws(() => ghosting.restoreRegistry(broken, root), { code: "REGISTRY_FAILED" });
  assert.ok(ghosting.readMarker(root));
  broken.failImport = false;
  ghosting.restoreRegistry(broken, root);
  assert.deepEqual(broken.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 800 });
});

test("сбой посреди рендера: при старте реестр возвращается по маркеру, mute помечается к снятию", (t) => {
  const root = tempDir(t);
  const reg = fakeRegistry({ ScreenWidth: 2560 });
  ghosting.snapshotRegistry(reg, root, { muted: true });
  reg.keys.set(ghosting.REGISTRY_KEY, { ScreenWidth: 1280 });
  const store = fx.memoryStore();
  const adapter = new ghosting.GhostingAdapter({ jobsRoot: root, registry: reg, store, script: "x" });
  assert.equal(adapter.recover().muted, true);
  assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(store.get(ghosting.UNMUTE_PENDING), true);
  assert.equal(ghosting.readMarker(root), null);
  assert.equal(adapter.recover(), null);

  // DemoTool делает это при создании, до чистки старых папок заданий.
  ghosting.snapshotRegistry(reg, root, {});
  reg.keys.set(ghosting.REGISTRY_KEY, { ScreenWidth: 1 });
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: root, ghostingAdapter: new ghosting.GhostingAdapter({ jobsRoot: root, registry: reg, script: "x" }) });
  assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(tool.renderActive("ghosting-renewed"), false);
});

test("папки рендера в моде: удаляются только внутри hlsr-render, по ссылкам не ходим", (t) => {
  const dir = tempDir(t);
  const mod = path.join(dir, "ghostingmod");
  const outside = path.join(dir, "outside");
  fs.mkdirSync(path.join(mod, "hlsr-render", "old1"), { recursive: true });
  fs.mkdirSync(outside);
  fs.writeFileSync(path.join(outside, "keep.txt"), "x");
  fs.writeFileSync(path.join(mod, "hlsr-render", "old1", "f_0001.tga"), "x");
  const type = process.platform === "win32" ? "junction" : "dir";
  fs.symlinkSync(outside, path.join(mod, "hlsr-render", "old1", "link"), type);
  fs.symlinkSync(outside, path.join(mod, "hlsr-render", "link2"), type);
  fs.writeFileSync(path.join(mod, "keep.cfg"), "x");
  assert.equal(ghosting.cleanupRenderDirs(mod, null), 2);
  assert.equal(fs.existsSync(path.join(mod, "hlsr-render")), false);
  assert.ok(fs.existsSync(path.join(outside, "keep.txt")));
  assert.ok(fs.existsSync(path.join(mod, "keep.cfg")));

  // Сама hlsr-render — ссылка: снимается ссылка, цель цела.
  fs.symlinkSync(outside, path.join(mod, "hlsr-render"), type);
  ghosting.cleanupRenderDirs(mod, null);
  assert.equal(fs.existsSync(path.join(mod, "hlsr-render")), false);
  assert.ok(fs.existsSync(path.join(outside, "keep.txt")));
});

test("возможности: разрешение не больше основного монитора, 4K только от 3840×2160", () => {
  assert.equal(ghosting.maxHeightFor({ width: 2560, height: 1440 }), 1440);
  assert.equal(ghosting.maxHeightFor({ width: 1920, height: 1080 }), 1080);
  assert.equal(ghosting.maxHeightFor({ width: 3840, height: 2160 }), 2160);
  assert.equal(ghosting.maxHeightFor({ width: 3440, height: 1440 }), 1440);
  assert.equal(ghosting.maxHeightFor({ width: 1366, height: 768 }), 720);
  const adapter = new ghosting.GhostingAdapter({ screenSize: () => ({ width: 2560, height: 1440 }), script: "x" });
  assert.deepEqual(adapter.capabilities, { maxHeight: 1440, fps: [30, 60, 120], keyhud: true, audio: true, integrity: true, ownConfig: true, ownHud: true });
});

// ---------- адаптер целиком: подставная игра ----------

const W = 8;
const H = 4;
// Подставная игра за помощником: разбирает командную строку, как Source, и
// пишет в папку мода журнал, кадры TGA (первый байт — сегмент, второй — номер
// кадра в нём) и WAV с опережением микшера; после последнего сегмента
// начинает второй круг startdemos и ждёт закрытия. Как помощник: сразу
// сообщает о глушении, на stop закрывает «игру» и отвечает stopped с pending
// (mute остался, если без сброса или сброс не удался); может «погибнуть»
// (helperDiesAfter) или «выйти сама» (gameDiesAfter) после N кадров.
class FakeHost extends EventEmitter {
  constructor(game) {
    super();
    this.game = game;
    this.exitCode = null;
    this.closed = false;
    this.stopFlag = false;
    this.pending = null;
  }

  async start(request) {
    this.request = request;
    this.pid = null;
    this.game.launches.push(request);
    // noSessionAt: начиная с этого запуска игра не успевает открыть звуковую сессию.
    this.session = !(this.game.noSessionAt && this.game.launches.length >= this.game.noSessionAt);
    this.running = this.simulate(argv(request.commandLine)).catch((error) => {
      this.game.errors.push(error);
    });
    this.emit("event", { event: "started", pid: this.pid });
    if (this.session) this.emit("event", { event: "audio", action: "muted", pid: 1, device: 0, was: request.unmuteAll });
    return this.pid;
  }

  async simulate(args) {
    const { modDir, fps, framesBy, gapMs, failAt } = this.game;
    const value = (name) => args[args.indexOf(name) + 1];
    const log = path.join(modDir, ...value("+con_logfile").split("/"));
    const demos = value("+startdemos").split(" ");
    const [movie, ...formats] = value("+startmovie").split(" ");
    const dir = path.join(modDir, ...movie.split("/").slice(0, -1));
    const prefix = movie.split("/").pop();
    const wav = formats.includes("wav") && !this.game.noWav ? path.join(dir, `${prefix}.wav`) : null;
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const say = (line) => fs.appendFileSync(log, `${line}\n`);
    say(`${demos.length} demo(s) in loop`);
    if (wav) {
      const header = sr.wavHeader({ channels: 2, rate: 44100, byteRate: 176400, blockAlign: 4, bits: 16 }, 0);
      header.writeUInt32LE(0, 4);
      fs.writeFileSync(wav, Buffer.concat([header, Buffer.alloc(Math.round(sr.mixerLead(fps) * 44100) * 4, 1)]));
    }
    const size = this.game.frameSize || { width: W, height: H };
    let frame = 0;
    for (let round = 0; ; round++) {
      for (const name of demos) {
        say(`Playing demo from ${name}.dem.`);
        if (round === 1) {
          while (!this.stopFlag) await sleep(10);
          return;
        }
        await sleep(gapMs);
        const segmentNo = Number(name.split("/").pop().slice(1));
        if (failAt === segmentNo) {
          say("Missing map maps/d1_canals_01a.bsp,  disconnecting");
          while (!this.stopFlag) await sleep(10);
          return;
        }
        for (let i = 0; i < framesBy[segmentNo]; i++) {
          if (this.stopFlag) return;
          if (this.game.helperDiesAfter !== undefined && this.game.written >= this.game.helperDiesAfter) {
            // Помощник погиб: задание закрыло игру, события stopped не будет.
            this.closed = true;
            this.emit("closed", { code: 1 });
            return;
          }
          if (this.game.gameDiesAfter !== undefined && this.game.written >= this.game.gameDiesAfter) {
            this.exitCode = 3;
            this.emit("exited", { code: 3 });
            return;
          }
          fs.writeFileSync(path.join(dir, `${prefix}${String(frame++).padStart(4, "0")}.tga`), tga(size.width, size.height, Buffer.from([segmentNo, i])));
          if (wav) fs.appendFileSync(wav, Buffer.alloc(Math.round(44100 / fps) * 4, 2));
          this.game.written++;
          await sleep(3);
        }
      }
    }
  }

  get helperPid() {
    return null;
  }

  stop(reset) {
    if (this.stopping) return this.stopping;
    this.stopFlag = true;
    this.game.stops++;
    this.game.resets.push(reset !== false);
    this.stopping = this.running.then(() => {
      if (this.closed) return;
      // Как помощник: сброс — только если сессии были (глушили сами).
      if (this.session && reset !== false) {
        const ok = this.game.resetOk !== false;
        this.emit("event", { event: "audio-reset", ok });
        this.pending = !ok;
      // Без сессии — pending:false, как помощник на stop до старта игры: адаптер не должен верить этому как сбросу.
      } else this.pending = this.session;
      this.emit("event", { event: "stopped", code: 1, pending: this.pending });
    });
    return this.stopping;
  }

  detach() {
    this.stopFlag = true;
    this.game.detached = true;
  }

  kill() {
    this.stopFlag = true;
  }
}
// Подставной FFmpeg: принимает кадры в stdin, запоминает первые байты каждого.
function fakeFfmpeg(sink) {
  return (exe, args) => {
    const child = new EventEmitter();
    child.exitCode = null;
    child.args = args;
    child.stderr = Readable.from([]);
    child.stdin = new Writable({
      write(chunk, encoding, callback) {
        sink.frames.push([chunk[0], chunk[1], chunk.length]);
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
    sink.args = args;
    return child;
  };
}

function setupAdapter(t, options) {
  const opts = options || {};
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  const modDir = path.join(install, "ghostingmod");
  fs.mkdirSync(modDir, { recursive: true });
  fs.writeFileSync(path.join(install, "hl2.exe"), "");
  // 50, 1 тик (пропуск), 30, 40 тиков на 30 к/с: 15, 9, 12 кадров.
  const playlist = fx.writeGhostingRun(path.join(modDir, "demos"), "run1", opts.run || [
    { map: "m1", duration: 0.5 }, { map: "m1", duration: 0.01 }, { map: "m1", duration: 0.3 }, { map: "m2", duration: 0.4 },
  ]);
  const item = library.describe({ kind: "playlist", files: [playlist], name: "run1" });
  const jobsRoot = path.join(dir, "jobs");
  const jobDir = path.join(jobsRoot, "abcdef0123456789");
  fs.mkdirSync(jobDir, { recursive: true });
  const reg = fakeRegistry({ ScreenWidth: 2560 });
  const game = { modDir, fps: 30, framesBy: { 1: 15, 2: 9, 3: 12 }, gapMs: 300, launches: [], errors: [], written: 0, stops: 0, resets: [], failAt: opts.failAt };
  const sink = { frames: [] };
  const store = fx.memoryStore();
  if (opts.unmutePending) store.set(ghosting.UNMUTE_PENDING, true);
  const adapter = new ghosting.GhostingAdapter(Object.assign({
    platform: "win32",
    jobsRoot,
    store,
    registry: reg,
    script: "render-host.ps1",
    installPath: () => install,
    gameRunning: () => false,
    steamStatus: () => (game.steamDown ? { ok: false, code: "STEAM_NOT_RUNNING" } : { ok: true }),
    inspector: { findByImageName: () => [], samePath: (a, b) => a === b },
    freeSpace: () => 1e15,
    hostFactory: () => new FakeHost(game),
    spawnFn: fakeFfmpeg(sink),
    batchSize: 2,
  }, opts.adapter || {}));
  const request = Object.assign({ item, range: { from: 0, to: item.duration }, fps: 30, width: W, height: H, audio: true, keyhud: true,
    jobDir, ffmpegPath: "ffmpeg", onProgress: () => {} }, opts.request || {});
  return { dir, install, modDir, item, jobsRoot, jobDir, reg, game, sink, store, adapter, request };
}

test("адаптер: пачки подряд, насос кадров, стыки, звук ровно под кадры, реестр и папки возвращены", { timeout: 60000 }, async (t) => {
  const { adapter, request, sink, game, reg, modDir, jobsRoot, jobDir, store } = setupAdapter(t, { unmutePending: true });
  const progress = [];
  request.onProgress = (info) => progress.push(info);
  assert.deepEqual(adapter.canRender(request.item), { ok: true });
  let activeDuring = false;
  const pending = adapter.render(request);
  setTimeout(() => { activeDuring = adapter.isActive(); }, 50);
  const material = await pending;
  assert.equal(activeDuring, true);
  assert.equal(adapter.isActive(), false);
  assert.deepEqual(game.errors, []);

  // Две пачки по 2 и 1 сегменту (пустой пропущен), игра закрыта после каждой.
  assert.equal(game.launches.length, 2);
  assert.equal(game.stops, 2);
  // Игра всегда закрывается заглушённой; mute снимается сбросом только в конце рендера.
  assert.deepEqual(game.resets, [false, true]);
  assert.equal(game.launches[0].mute, true);
  assert.equal(game.launches[0].unmuteAll, true, "mute, оставшийся после сбоя, снимается");
  assert.equal(game.launches[1].unmuteAll, true, "вторая пачка стартует с mute первой — он наш");
  assert.equal(store.get(ghosting.UNMUTE_PENDING), false);
  assert.match(game.launches[0].commandLine, /\+startdemos "hlsr-render\/abcdef01\/s001 hlsr-render\/abcdef01\/s002"/);
  assert.match(game.launches[1].commandLine, /\+startdemos hlsr-render\/abcdef01\/s003 /);
  assert.match(game.launches[0].resetCommandLine, /\+volume 0/);

  // Кадры: 15 + 9 + 12, ни одного кадра второго круга. Первый кадр видео —
  // копия второго, первый кадр каждого следующего сегмента — копия последнего кадра прошлого.
  const frames = sink.frames.map(([segment, index]) => `${segment}:${index}`);
  assert.equal(frames.length, 36);
  assert.deepEqual(frames.slice(0, 3), ["1:1", "1:1", "1:2"]);
  assert.deepEqual(frames.slice(14, 17), ["1:14", "1:14", "2:1"]);
  assert.deepEqual(frames.slice(23, 26), ["2:8", "2:8", "3:1"]);
  assert.equal(frames[35], "3:11");
  assert.ok(sink.frames.every(([, , length]) => length === W * H * 3));
  assert.ok(sink.args.includes("vflip"), "TGA снизу вверх");
  assert.deepEqual(sink.args.slice(sink.args.indexOf("-c:v"), sink.args.indexOf("-c:v") + 6), ["-c:v", "libx264", "-preset", "ultrafast", "-crf", "14"]);

  // Звук: опережение отрезано, ровно 36 кадров / 30 к/с.
  const wav = fs.readFileSync(material.audio);
  assert.equal(wav.readUInt32LE(40), Math.round(36 / 30 * 44100) * 4);
  assert.equal(wav.readInt16LE(44), 0x0202, "первая выборка — уже звук кадров, не опережение");
  assert.ok(material.video.startsWith(jobDir) && material.audio.startsWith(jobDir));
  assert.equal(material.offset, 0);
  assert.deepEqual(material.cuts, [{ from: 0.5, to: 0.51 }]);
  assert.deepEqual(material.timeline.filter((m) => m.type !== "timer").map((m) => [m.t, m.type, m.label]), [[0, "map", "m1"], [0.51, "saveload", "3"], [0.81, "map", "m2"]]);
  assert.ok(progress.some((info) => info.segment && info.segment.index === 3 && info.segment.count === 3));
  assert.ok(progress.every((info) => info.fraction >= 0 && info.fraction < 1));

  assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.deepEqual(reg.calls, ["export", "delete", "import"]);
  assert.equal(fs.existsSync(path.join(modDir, "hlsr-render")), false);
  assert.deepEqual(fs.readdirSync(jobsRoot), ["abcdef0123456789"]);
});

test("адаптер: надпись «GH DEMO DATA: INTACT» доходит до командной строки игры только по выбору", { timeout: 60000 }, async (t) => {
  for (const [integrity, expected] of [[true, "1"], [false, "0"], [undefined, "0"]]) {
    const setup = setupAdapter(t, { adapter: { batchSize: 32 }, request: { integrity } });
    await setup.adapter.render(setup.request);
    const args = argv(setup.game.launches[0].commandLine);
    assert.equal(args[args.indexOf("+gh_demo_overlay") + 1], expected, String(integrity));
  }
  assert.equal(new ghosting.GhostingAdapter({ script: "x" }).capabilities.integrity, true, "Ghosting её умеет — переключатель в окне есть");
});
test("адаптер: интервал из начала длинного сегмента — игра закрывается, как только кадров хватает", { timeout: 60000 }, async (t) => {
  const { adapter, request, sink, game } = setupAdapter(t, {
    run: [{ map: "m1", duration: 2 }, { map: "m1", duration: 0.3 }],
    request: { range: { from: 0, to: 0.5 } },
  });
  game.framesBy = { 1: 60, 2: 9 };
  const material = await adapter.render(request);
  assert.equal(game.launches.length, 1);
  assert.deepEqual(game.resets, [true], "кадров хватило — конец рендера, со сбросом mute");
  assert.ok(sink.frames.length >= 17 && sink.frames.length < 40, String(sink.frames.length));
  // Закрыли посреди сегмента: опережение звука — по формуле, длина — ровно под кадры.
  const wav = fs.readFileSync(material.audio);
  assert.equal(wav.readUInt32LE(40), Math.round(sink.frames.length / 30 * 44100) * 4);
  assert.equal(wav.readInt16LE(44), 0x0202);
});

test("адаптер: отмена посреди рендера — игра закрыта, реестр возвращён, папка в моде удалена", { timeout: 60000 }, async (t) => {
  const { adapter, request, game, reg, modDir, store } = setupAdapter(t);
  const controller = new AbortController();
  request.signal = controller.signal;
  request.onProgress = (info) => {
    if (info.fraction > 0.1) controller.abort();
  };
  await assert.rejects(adapter.render(request), { code: "CANCELLED" });
  assert.ok(game.stops >= 1);
  assert.deepEqual(game.resets, [true]);
  assert.equal(store.get(ghosting.UNMUTE_PENDING), undefined, "сброс прошёл — флага нет");
  assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(modDir, "hlsr-render")), false);
  assert.equal(adapter.isActive(), false);
});

test("выход лаунчера посреди рендера: синхронно — помощнику stop, реестр возвращён, папка в моде удалена", { timeout: 60000 }, async (t) => {
  const { adapter, request, game, reg, modDir, store } = setupAdapter(t);
  game.framesBy = { 1: 300, 2: 9, 3: 12 };
  const controller = new AbortController();
  request.signal = controller.signal;
  let checked = false;
  request.onProgress = (info) => {
    if (checked || info.fraction < 0.02) return;
    checked = true;
    // Как DemoTool.dispose: отмена и сразу синхронная уборка.
    controller.abort();
    adapter.disposeSync();
    assert.equal(game.detached, true);
    assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
    assert.equal(fs.existsSync(path.join(modDir, "hlsr-render")), false);
    // События помощника после выхода лаунчера не прочесть: mute помечен к снятию следующим рендером.
    assert.equal(store.get(ghosting.UNMUTE_PENDING), true);
  };
  await assert.rejects(adapter.render(request), { code: "CANCELLED" });
  assert.equal(checked, true);
  assert.equal(adapter.isActive(), false);
});
test("адаптер: нет карты (журнал консоли) — понятный код, всё возвращено", { timeout: 60000 }, async (t) => {
  const { adapter, request, reg, modDir } = setupAdapter(t, { failAt: 2 });
  await assert.rejects(adapter.render(request), { code: "MAP_MISSING" });
  assert.deepEqual(reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(modDir, "hlsr-render")), false);
});

test("адаптер: нет новых кадров — RENDER_TIMEOUT", { timeout: 60000 }, async (t) => {
  const { adapter, request, game } = setupAdapter(t, { adapter: { firstFrameTimeoutMs: 400 } });
  game.gapMs = 1500;
  await assert.rejects(adapter.render(request), { code: "RENDER_TIMEOUT" });
});

test("адаптер: предусловия — игра запущена, Steam, место, изменённый сегмент, не установлена", async (t) => {
  const running = setupAdapter(t, { adapter: { gameRunning: (id) => id === "ghosting-renewed" } });
  await assert.rejects(running.adapter.render(running.request), { code: "GAME_RUNNING" });
  const outside = setupAdapter(t);
  outside.adapter.inspector = { findByImageName: () => [{ pid: 5, executablePath: path.join(outside.install, "hl2.exe") }], samePath: (a, b) => a === b };
  await assert.rejects(outside.adapter.render(outside.request), { code: "GAME_RUNNING" });
  const steam = setupAdapter(t, { adapter: { steamStatus: () => ({ ok: false, code: "STEAM_NOT_RUNNING" }) } });
  await assert.rejects(steam.adapter.render(steam.request), { code: "STEAM_NOT_RUNNING" });
  const space = setupAdapter(t, { adapter: { freeSpace: () => 1024 } });
  await assert.rejects(space.adapter.render(space.request), { code: "NO_SPACE" });
  const changed = setupAdapter(t);
  const file = changed.item.segments[2].path;
  const data = fs.readFileSync(file);
  data[data.length - 1] ^= 1;
  fs.writeFileSync(file, data);
  await assert.rejects(changed.adapter.render(changed.request), { code: "SEGMENT_CHANGED", details: { segment: 3 } });
  for (const setup of [running, outside, steam, space, changed]) {
    assert.equal(setup.game.launches.length, 0);
    assert.equal(fs.existsSync(path.join(setup.modDir, "hlsr-render")), false);
    assert.deepEqual(setup.reg.calls, []);
  }
  const missing = new ghosting.GhostingAdapter({ platform: "win32", installPath: () => null, script: "x" });
  assert.deepEqual(missing.canRender(changed.item), { ok: false, code: "GAME_NOT_INSTALLED" });
  assert.deepEqual(new ghosting.GhostingAdapter({ platform: "linux", installPath: () => changed.install, script: "x" }).canRender(changed.item), { ok: false, code: "RENDER_NOT_READY" });
});

// I-1: mute игрока после любого конца рендера либо снят, либо помечен флагом.
test("mute игрока: каждый путь завершения — снят сбросом или помечен к снятию", { timeout: 120000 }, async (t) => {
  const pending = (setup) => setup.store.get(ghosting.UNMUTE_PENDING);
  const cases = [
    { name: "успех", expect: undefined },
    { name: "ошибка (нет карты)", options: { failAt: 2 }, code: "MAP_MISSING", expect: undefined },
    { name: "помощник погиб", game: { helperDiesAfter: 5 }, code: "RENDER_HOST_FAILED", expect: true },
    { name: "игра вышла сама, сброс удался", game: { gameDiesAfter: 5 }, code: "GAME_EXITED", expect: undefined },
    { name: "игра вышла сама, сброс не удался", game: { gameDiesAfter: 5, resetOk: false }, code: "GAME_EXITED", expect: true },
    { name: "сброс в конце не удался", game: { resetOk: false }, expect: true },
    { name: "Steam закрыли между пачками", game: { steamAfterFirst: true }, code: "STEAM_NOT_RUNNING", expect: true },
  ];
  for (const item of cases) {
    const setup = setupAdapter(t, item.options);
    Object.assign(setup.game, item.game || {});
    if (setup.game.steamAfterFirst) setup.request.onProgress = (info) => { if (info.segment && info.segment.index === 2) setup.game.steamDown = true; };
    const run = setup.adapter.render(setup.request);
    if (item.code) await assert.rejects(run, { code: item.code }, item.name);
    else await run;
    assert.equal(pending(setup), item.expect, item.name);
    assert.equal(ghosting.readMarker(setup.jobsRoot), null, `${item.name}: маркер снят`);
    assert.deepEqual(setup.reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 }, `${item.name}: реестр`);
  }

  // Флаг с прошлого раза: следующий рендер считает заглушённые сессии своими и снимает флаг.
  const next = setupAdapter(t, { unmutePending: true });
  await next.adapter.render(next.request);
  assert.equal(next.game.launches[0].unmuteAll, true);
  assert.equal(pending(next), false);
  // Сбой лаунчера посреди рендера: маркер с muted — флаг при следующем старте.
  const crash = setupAdapter(t);
  ghosting.snapshotRegistry(crash.reg, crash.jobsRoot, { muted: true });
  new ghosting.GhostingAdapter({ jobsRoot: crash.jobsRoot, registry: crash.reg, store: crash.store, script: "x" }).recover();
  assert.equal(pending(crash), true);
});

// N-1: «сессию не видели» — не сброс. Флаг снимается только после подтверждённого сброса mute.
test("mute игрока: игра не успела открыть звук (отмена или падение в начале пачки) — флаг остаётся", { timeout: 120000 }, async (t) => {
  const pending = (setup) => setup.store.get(ghosting.UNMUTE_PENDING);

  // Пачка 1 закрыта заглушённой, во второй — отмена до появления звука.
  const cancelled = setupAdapter(t);
  cancelled.game.noSessionAt = 2;
  const controller = new AbortController();
  cancelled.request.signal = controller.signal;
  cancelled.request.onProgress = (info) => { if (info.segment && info.segment.index === 3) controller.abort(); };
  await assert.rejects(cancelled.adapter.render(cancelled.request), { code: "CANCELLED" });
  assert.equal(cancelled.game.launches.length, 2);
  assert.equal(cancelled.game.launches[1].unmuteAll, true);
  assert.equal(pending(cancelled), true, "mute первой пачки не снят — флаг");

  // Игра второй пачки упала до звука.
  const crashed = setupAdapter(t);
  Object.assign(crashed.game, { noSessionAt: 2, gameDiesAfter: 24 });
  await assert.rejects(crashed.adapter.render(crashed.request), { code: "GAME_EXITED" });
  assert.equal(pending(crashed), true);

  // Флаг с прошлого раза, одна пачка: отмена до звука — флаг не теряется.
  const early = setupAdapter(t, { unmutePending: true, adapter: { batchSize: 32 } });
  early.game.noSessionAt = 1;
  const stop = new AbortController();
  early.request.signal = stop.signal;
  early.request.onProgress = () => stop.abort();
  await assert.rejects(early.adapter.render(early.request), { code: "CANCELLED" });
  assert.equal(pending(early), true);

  // Флаг с прошлого раза, рендер прошёл, но звуковой сессии не было (нет устройства) — флаг тоже остаётся.
  const silent = setupAdapter(t, { unmutePending: true, adapter: { batchSize: 32 } });
  silent.game.noSessionAt = 1;
  await silent.adapter.render(silent.request);
  assert.equal(pending(silent), true);
  assert.deepEqual(silent.game.resets, [true]);

  // А со звуком — подтверждённый сброс снимает флаг.
  const healed = setupAdapter(t, { unmutePending: true, adapter: { batchSize: 32 } });
  await healed.adapter.render(healed.request);
  assert.equal(pending(healed), false);
});
test("старый снимок реестра: сначала возвращается он, не перезаписывается; не вернулся — рендера нет", { timeout: 60000 }, async (t) => {
  const setup = setupAdapter(t);
  // Прошлый рендер упал: снимок игрока (2560) в маркере, в реестре — значения рендера.
  ghosting.snapshotRegistry(setup.reg, setup.jobsRoot, { muted: false });
  setup.reg.keys.set(ghosting.REGISTRY_KEY, { ScreenWidth: 640 });
  assert.throws(() => ghosting.snapshotRegistry(setup.reg, setup.jobsRoot), { code: "REGISTRY_FAILED" }, "поверх невозвращённого — нельзя");
  await setup.adapter.render(setup.request);
  assert.deepEqual(setup.reg.keys.get(ghosting.REGISTRY_KEY), { ScreenWidth: 2560 });

  const broken = setupAdapter(t);
  ghosting.snapshotRegistry(broken.reg, broken.jobsRoot, {});
  broken.reg.keys.set(ghosting.REGISTRY_KEY, { ScreenWidth: 640 });
  broken.reg.failImport = true;
  await assert.rejects(broken.adapter.render(broken.request), { code: "REGISTRY_FAILED" });
  assert.equal(broken.game.launches.length, 0);
  assert.ok(ghosting.readMarker(broken.jobsRoot), "маркер и снимок остались");
  assert.ok(fs.existsSync(path.join(broken.jobsRoot, ghosting.REG_FILE)));
});

test("reg.exe: «ключа нет» — только когда родитель читается без него; сбой — REGISTRY_FAILED", () => {
  const key = ghosting.REGISTRY_KEY;
  const parent = "HKCU\\Software\\Valve\\Source\\ghostingmod";
  const listing = (children) => ({ status: 0, stdout: `\r\nHKEY_CURRENT_USER\\Software\\Valve\\Source\\ghostingmod\r\n${children.map((c) => `HKEY_CURRENT_USER\\Software\\Valve\\Source\\ghostingmod\\${c}`).join("\r\n")}\r\n` });
  const tool = (answers) => ghosting.regTool({}, (args) => {
    const answer = answers[`${args[0]} ${args[1]}`];
    return typeof answer === "function" ? answer() : answer || { status: 1, stdout: "" };
  });
  assert.equal(tool({ [`query ${key}`]: { status: 0 } }).exists(key), true);
  assert.equal(tool({ [`query ${parent}`]: listing(["Other"]) }).exists(key), false);
  // Таймаут или запрет: родитель читается, ключ в нём есть — значит, не прочли сам ключ.
  assert.throws(() => tool({ [`query ${key}`]: { status: null }, [`query ${parent}`]: listing(["Settings"]) }).exists(key), { code: "REGISTRY_FAILED" });
  // Родителя нет — ключа тем более (выше по дереву родитель читается).
  assert.equal(tool({ "query HKCU\\Software\\Valve\\Source": { status: 0, stdout: "HKEY_CURRENT_USER\\Software\\Valve\\Source\\hl2\r\n" } }).exists(key), false);
  // Ничего не читается до самого HKCU — сбой, а не «нет ключа».
  assert.throws(() => tool({}).exists(key), { code: "REGISTRY_FAILED" });
  // delete: не удалось, а ключ есть — сбой; не удалось, потому что ключа нет, — успех.
  assert.throws(() => tool({ [`delete ${key}`]: { status: 1 }, [`query ${key}`]: { status: 0 } }).deleteKey(key), { code: "REGISTRY_FAILED" });
  tool({ [`delete ${key}`]: { status: 1 }, [`query ${parent}`]: listing([]) }).deleteKey(key);
  // Снимок при сбое reg — до любых изменений: игра не запускается.
  const reg = { exists() { throw Object.assign(new Error("reg"), { name: "DemoError", code: "REGISTRY_FAILED" }); } };
  assert.throws(() => ghosting.snapshotRegistry(reg, os.tmpdir()), { code: "REGISTRY_FAILED" });
});

test("адаптер: кадр игры меньше запрошенного (масштаб экрана) — RESOLUTION_LIMITED с фактическим размером", { timeout: 60000 }, async (t) => {
  const setup = setupAdapter(t);
  setup.game.frameSize = { width: 6, height: 4 };
  await assert.rejects(setup.adapter.render(setup.request), { code: "RESOLUTION_LIMITED", details: { width: 6, height: 4 } });
  assert.equal(setup.sink.frames.length, 0);
  assert.equal(fs.existsSync(path.join(setup.modDir, "hlsr-render")), false);
});

test("адаптер: игра не записала звук — видео без звука, а не отказ", { timeout: 60000 }, async (t) => {
  const setup = setupAdapter(t);
  setup.game.noWav = true;
  const material = await setup.adapter.render(setup.request);
  assert.equal(material.audio, null);
  assert.equal(setup.sink.frames.length, 36);
  assert.equal(fs.existsSync(path.join(setup.jobDir, "audio.wav")), false);
});

test("адаптер: игра занята установкой или ремонтом — GAME_BUSY", async (t) => {
  const setup = setupAdapter(t, { adapter: { gameBusy: () => true } });
  await assert.rejects(setup.adapter.render(setup.request), { code: "GAME_BUSY" });
  assert.equal(setup.game.launches.length, 0);
});

// Помощник: C# компилируется только при запуске (Add-Type) — проверка без
// стола и игры: ready, stop, stopped.
test("помощник рендера запускается и отвечает по протоколу", { timeout: 120000 }, async (t) => {
  if (process.platform !== "win32") return t.skip("ПРОПУЩЕНО: помощник только для Windows");
  const { spawn } = require("node:child_process");
  const { scriptPath } = require("../src/main/demo-tool/render-host");
  const { powershellPath } = require("../src/main/windows-tools");
  const child = spawn(powershellPath(), ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", scriptPath(null)],
    { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  const events = [];
  let buffer = "";
  const done = new Promise((resolve) => child.on("close", resolve));
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at).trim();
      buffer = buffer.slice(at + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      events.push(message);
      if (message.event === "ready") child.stdin.write(`${JSON.stringify({ cmd: "stop" })}\n`);
    }
  });
  const code = await done;
  assert.equal(code, 0);
  assert.deepEqual(events.map((message) => message.event), ["ready", "stopped"]);
  assert.equal(events[1].pending, false);
});
test("запуск Ghosting из лаунчера во время рендера — RENDER_ACTIVE", async () => {
  const electronPath = require.resolve("electron");
  const channels = new Map();
  require.cache[electronPath] = { id: electronPath, filename: electronPath, loaded: true, exports: { ipcMain: { handle: (channel, fn) => channels.set(channel, fn), on() {} } } };
  const GameDelivery = require("../src/main/game-delivery");
  const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
  const delivery = new GameDelivery({});
  let launched = 0;
  delivery.service = { isGameRunning: () => false };
  delivery.telemetryGameStarting = () => {};
  delivery.launchService = { launch: async () => { launched++; return { status: "started" }; } };
  delivery.ensurePracticeSaves = () => {};
  let rendering = true;
  delivery.renderActive = (gameId) => rendering && gameId === "ghosting-renewed";
  await assert.rejects(delivery.launch("ghosting-renewed"), { code: "RENDER_ACTIVE" });
  assert.equal(launched, 0);
  assert.equal((await delivery.launch("hl1-won")).status, "started");
  rendering = false;
  assert.equal((await delivery.launch("ghosting-renewed")).status, "started");
  // Установка, обновление, ремонт, удаление (reserveSlot) и проверка «игра не запущена» во время рендера — тот же код.
  const service = Object.create(GameDeliveryService.prototype);
  Object.assign(service, { slot: null, renderActive: (gameId) => gameId === "ghosting-renewed", isBusy: () => false,
    processService: { isRunning: () => false } });
  for (const operation of ["install", "update", "repair", "uninstall", "import"])
    assert.throws(() => service.reserveSlot("ghosting-renewed", operation), { code: "RENDER_ACTIVE" }, operation);
  assert.throws(() => service.assertGameNotRunning("ghosting-renewed"), { code: "RENDER_ACTIVE" });
  assert.equal(service.reserveSlot("hl1-won", "install").gameId, "hl1-won");
  // Проводка до настоящих обработчиков IPC: удаление и ремонт Ghosting во время рендера.
  const wired = new GameDelivery({});
  wired.service = wired.guardRendering(Object.assign(Object.create(GameDeliveryService.prototype), {
    slot: null, isBusy: () => false, processService: { isRunning: () => false }, installService: { readInstalled: () => null },
  }));
  wired.renderActive = (gameId) => gameId === "ghosting-renewed";
  wired.initialize();
  for (const channel of ["game-menu-uninstall", "game-menu-repair"]) {
    const answer = await channels.get(channel)({}, "ghosting-renewed", { confirmed: true });
    assert.equal(answer.status, "error", channel);
    assert.equal(answer.error.code, "RENDER_ACTIVE", channel);
  }
  // ensureService ставит ту же защиту на настоящий сервис.
  const source = fs.readFileSync(path.join(__dirname, "..", "src", "main", "game-delivery", "index.js"), "utf8");
  const ensure = source.slice(source.indexOf("  ensureService() {"), source.indexOf("this.attachProcessEvents();"));
  assert.match(ensure, /this\.guardRendering\(this\.service\);/);  // DemoTool отвечает за проверку: true только пока адаптер рендерит.
  const tool = new DemoTool({ store: fx.memoryStore(), ghostingAdapter: { isActive: () => true, recover() {}, disposeSync() {} } });
  assert.equal(tool.renderActive("ghosting-renewed"), true);
  assert.equal(tool.renderActive("hl1-won"), false);
});

// Настоящая игра: копия Ghosting с ранами в ghostingmod/demos (не установка
// игрока!), Steam запущен. Рендер первых 3 с в 720p30 со звуком на скрытом
// столе через настоящий помощник; реестр и папки — как были.
test("настоящая игра: 3 с рана на скрытом столе (HLSR_DEMO_GHOSTING_GAME)", { timeout: 300000 }, async (t) => {
  const game = process.env.HLSR_DEMO_GHOSTING_GAME;
  const ff = fx.findFfmpeg();
  if (!game || !ff || process.platform !== "win32")
    return t.skip("ПРОПУЩЕНО: нужна копия Ghosting с ранами (HLSR_DEMO_GHOSTING_GAME), FFmpeg (HLSR_DEMO_FFMPEG), Windows и запущенный Steam");
  const { spawnSync } = require("node:child_process");
  const SteamLocator = require("../src/main/game-delivery/SteamLocator");
  const demos = path.join(game, "ghostingmod", "demos");
  const run = fs.readdirSync(demos).find((name) => fs.existsSync(path.join(demos, name, "run.ghplaylist")));
  assert.ok(run, "в копии нет рана с run.ghplaylist");
  const item = library.describe({ kind: "playlist", files: [path.join(demos, run, "run.ghplaylist")], name: run });
  const dir = tempDir(t);
  const jobDir = path.join(dir, "jobs", "0123456789abcdef");
  fs.mkdirSync(jobDir, { recursive: true });
  const query = () => spawnSync("reg", ["query", ghosting.REGISTRY_KEY], { encoding: "utf8", windowsHide: true }).stdout;
  const before = query();
  const adapter = new ghosting.GhostingAdapter({
    jobsRoot: path.join(dir, "jobs"),
    store: fx.memoryStore(),
    installPath: () => game,
    steamStatus: () => new SteamLocator({ logger: { info() {}, warn() {}, error() {} } }).describeSteamOnly(),
    screenSize: () => ({ width: 1280, height: 720 }),
  });
  const material = await adapter.render({ item, range: { from: 0, to: 3 }, fps: 30, width: 1280, height: 720, audio: true, keyhud: false,
    jobDir, ffmpegPath: ff.ffmpeg, onProgress: () => {} });
  const probe = JSON.parse(spawnSync(ff.ffprobe, ["-v", "error", "-count_frames", "-show_entries", "stream=width,height,nb_read_frames", "-of", "json", material.video], { encoding: "utf8" }).stdout);
  assert.equal(probe.streams[0].width, 1280);
  assert.ok(Number(probe.streams[0].nb_read_frames) >= 90, probe.streams[0].nb_read_frames);
  assert.ok(fs.statSync(material.audio).size > 44 + 3 * 176400 * 0.9);
  assert.equal(query(), before, "реестр игры вернулся");
  assert.equal(fs.existsSync(path.join(game, "ghostingmod", "hlsr-render")), false);
});
test("достижение «Монтажёр»: только за готовый экспорт, не за предпросмотр; сбой профиля не ломает задание", () => {
  const got = [];
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: os.tmpdir(), achieve: (id) => got.push(id) });
  tool.media = { register: () => ({ token: "t", url: "u" }) };
  const result = { file: "x.mp4", size: 1, attempts: 1, encoder: "libx264", warnings: [], timeline: { video: {}, markers: [] } };
  tool.finished("p1", "preview", Object.assign({ dir: "d" }, result));
  assert.deepEqual(got, []);
  tool.finished("e1", "export", result);
  assert.deepEqual(got, ["video-editor"]);
  const broken = new DemoTool({ store: fx.memoryStore(), jobsRoot: os.tmpdir(), achieve: () => { throw new Error("сеть"); } });
  broken.media = tool.media;
  assert.equal(broken.finished("e2", "export", result).kind, "export");
});

test("конфиг и HUD игрока в рендере Ghosting: свой — без лишних аргументов; стандартный — своя папка cfg впереди путей поиска", () => {
  const personalization = require("../src/main/game-delivery/ghosting-personalization");
  const dir = "ghostingmod/hlsr-render/ab/std";
  // Свой конфиг и свой HUD — как у игрока: ничего не добавляется.
  assert.deepEqual(sr.configArguments({ ownConfig: true, ownHud: true, dir }), { args: [], files: {} });
  assert.deepEqual(sr.configArguments({ dir }), { args: [], files: {} });
  // Свой конфиг, стандартный HUD: после autoexec игрока — значения «Персонализации» по умолчанию.
  const stockHud = sr.configArguments({ ownConfig: true, ownHud: false, dir });
  assert.deepEqual(stockHud.args, ["-insert_search_path", dir, "+exec", "hlsr-render-hud.cfg"]);
  assert.deepEqual(Object.keys(stockHud.files), ["cfg/hlsr-render-hud.cfg"], "config.cfg и autoexec.cfg игрока не заслоняются");
  assert.equal(stockHud.files["cfg/hlsr-render-hud.cfg"], personalization.toCfgLines(personalization.defaultPersonalization()));
  assert.match(stockHud.files["cfg/hlsr-render-hud.cfg"], /^gh_hud_color_enable "0"$/m);
  // Стандартный конфиг, свой HUD: пустые config.cfg и autoexec.cfg, «Персонализация» игрока — отдельно.
  const stockConfig = sr.configArguments({ ownConfig: false, ownHud: true, dir });
  assert.deepEqual(stockConfig.args, ["-insert_search_path", dir, "+exec", "hlsr-personalization.cfg"]);
  assert.deepEqual(Object.keys(stockConfig.files).sort(), ["cfg/autoexec.cfg", "cfg/config.cfg"]);
  for (const text of Object.values(stockConfig.files)) assert.ok(text.split("\n").every((line) => !line || line.startsWith("//")), "без настроек");
  // Всё стандартное: только пустые cfg.
  const stock = sr.configArguments({ ownConfig: false, ownHud: false, dir });
  assert.deepEqual(stock.args, ["-insert_search_path", dir]);
  assert.deepEqual(Object.keys(stock.files).sort(), ["cfg/autoexec.cfg", "cfg/config.cfg"]);
  // Старый путь: cfg исполняются раньше демок (до +startdemos и +startmovie).
  const args = sr.gameArguments({ width: 1280, height: 720, fps: 30, keyhud: true, audio: true, demos: ["x"], movie: "m", logFile: "l", config: stockConfig.args });
  assert.equal(args[args.indexOf("-insert_search_path") + 1], dir);
  assert.ok(args.indexOf("+exec") < args.indexOf("+startdemos"));
  assert.deepEqual(sr.gameArguments({ width: 1280, height: 720, fps: 30, keyhud: true, audio: true, demos: ["x"], movie: "m", logFile: "l", config: [] }),
    sr.gameArguments({ width: 1280, height: 720, fps: 30, keyhud: true, audio: true, demos: ["x"], movie: "m", logFile: "l" }));
});

test("адаптер: стандартный конфиг — cfg лежат в папке задания в моде на время запуска, потом уходят с ней", { timeout: 60000 }, async (t) => {
  const setup = setupAdapter(t, { request: { ownConfig: false, ownHud: false } });
  const std = path.join(setup.modDir, "hlsr-render", "abcdef01", "std", "cfg");
  const seen = [];
  setup.adapter.hostFactory = () => {
    const host = new FakeHost(setup.game);
    const start = host.start.bind(host);
    host.start = (request) => {
      seen.push(fs.readdirSync(std).sort());
      return start(request);
    };
    return host;
  };
  await setup.adapter.render(setup.request);
  assert.deepEqual(setup.game.errors, []);
  assert.ok(seen.length >= 1);
  for (const files of seen) assert.deepEqual(files, ["autoexec.cfg", "config.cfg"]);
  for (const launch of setup.game.launches) {
    const args = argv(launch.commandLine);
    assert.equal(args[args.indexOf("-insert_search_path") + 1], "ghostingmod/hlsr-render/abcdef01/std");
    assert.ok(!args.includes("+exec"), "свой HUD выключен — exec не нужен");
  }
  assert.equal(fs.existsSync(path.join(setup.modDir, "hlsr-render")), false, "папка задания в моде убрана");
});
