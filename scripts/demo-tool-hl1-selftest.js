"use strict";

// «Демо»: рендер Half-Life (WON) через bxt-rs — разбор демок (карты, кнопки,
// загрузки), сегменты и интервал, привязка видео ко времени демки, метки,
// KeyHUD (ASS), журнал игры, папка Injector задания, реестр и RENDER_ACTIVE,
// bxt-rs как скачиваемый инструмент. Настоящая игра — только по
// HLSR_DEMO_HL1_GAME (копия hl1-won) и HLSR_DEMO_HL1_DEMOS.

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { EventEmitter } = require("node:events");
const { spawnSync } = require("node:child_process");

const { scanDemo, BUTTONS, MAX_MAP_CHANGES } = require("../src/main/demo-tool/goldsrc-demo");
const headers = require("../src/main/demo-tool/headers");
const gsr = require("../src/main/demo-tool/goldsrc-render");
const goldsrc = require("../src/main/demo-tool/goldsrc");
const gr = require("../src/main/demo-tool/game-render");
const planner = require("../src/main/demo-tool/planner");
const library = require("../src/main/demo-tool/library");
const jobs = require("../src/main/demo-tool/jobs");
const { childEnv } = require("../src/main/demo-tool/render-host");
const DemoTool = require("../src/main/demo-tool");
const ToolService = require("../src/main/game-delivery/ToolService");
const toolRegistry = require("../src/main/game-delivery/tools");
const links = require("../src/shared/links");
const fx = require("./lib/demo-fixtures");

const ff = fx.findFfmpeg();
const NO_FFMPEG = "ПРОПУЩЕНО: нет FFmpeg — задайте HLSR_DEMO_FFMPEG (папка bin FFmpeg 9.0.2)";

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-hl1-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));
  return dir;
}

// Сетевой кадр с сообщением svc_serverinfo (в нём «maps/<карта>.bsp»).
const net = (time, frame, extra) => Object.assign({ type: 1, time, frame }, extra);
const serverinfo = (map) => `\x0b....maps/${map}.bsp\0....`;

// Сегмент 1: 2 с на c1a0, W с 0,5 до 1,0. Сегмент 2: 3 с, смена карты на
// c1a0d в 1,0 (клиент снова в игре в 1,2), прыжок в 2,0.
function runDemos(dir) {
  const write = (name, data) => {
    const file = path.join(dir, name);
    fs.writeFileSync(file, data);
    return file;
  };
  const one = write("run_1.dem", fx.goldsrcFramesDemo({
    duration: 2,
    loading: [{ type: 0, time: 0, message: serverinfo("c1a0") }],
    playback: [net(0, 0), net(0.05, 5), net(0.07, 7), { type: 4, time: 0.07 }, net(0.5, 50, { buttons: BUTTONS.forward }), net(1, 100), net(2, 200)],
  }));
  const two = write("run_2.dem", fx.goldsrcFramesDemo({
    duration: 3,
    loading: [{ type: 0, time: 0, message: serverinfo("c1a0") }],
    playback: [net(0, 0, { viewmodel: 3 }), net(0.08, 8, { viewmodel: 3 }), { type: 4, time: 0.08 }, net(1, 100, { message: serverinfo("c1a0d") }),
      { type: 3, time: 1.1 }, { type: 4, time: 1.2 }, net(2, 200, { buttons: BUTTONS.jump }), net(2.1, 210), net(3, 300)],
  }));
  return [one, two];
}

test("разбор демки: смены карт с паузой загрузки, кнопки, начало до 7-го кадра", async (t) => {
  const [one, two] = runDemos(tempDir(t));
  const a = await scanDemo(one);
  assert.equal(a.ok, true);
  assert.equal(a.broken, false);
  assert.equal(a.duration, 2);
  assert.deepEqual(a.mapChanges, [{ t: 0, map: "c1a0", gap: 0 }]);
  assert.deepEqual(a.buttons, [{ t: 0, buttons: 0 }, { t: 0.5, buttons: BUTTONS.forward }, { t: 1, buttons: 0 }]);
  assert.equal(a.startGap, 0.07, "без оружия — до 7-го кадра (правило bxt-rs)");
  const b = await scanDemo(two);
  assert.deepEqual(b.mapChanges, [{ t: 0, map: "c1a0", gap: 0 }, { t: 1, map: "c1a0d", gap: 0.2 }]);
  assert.equal(b.startGap, 0, "с оружием bxt-rs кадры начала не ждёт");
  assert.deepEqual(b.buttons.map((press) => press.buttons), [0, BUTTONS.jump, 0]);
});

test("разбор демки: битый и чужой файл — без исключений, прочитанное до сбоя остаётся", async (t) => {
  const dir = tempDir(t);
  const huge = path.join(dir, "huge.dem");
  fs.writeFileSync(huge, fx.goldsrcFramesDemo({ playback: [net(0.5, 1, { buttons: BUTTONS.duck }), net(1, 2, { length: 0x7fffffff })] }));
  const broken = await scanDemo(huge);
  assert.equal(broken.ok, true);
  assert.equal(broken.broken, true);
  assert.deepEqual(broken.buttons, [{ t: 0.5, buttons: BUTTONS.duck }]);

  // Обрезанный посреди кадра: директория указывает за конец — DEMO_DIRECTORY_INVALID.
  const cut = path.join(dir, "cut.dem");
  fs.writeFileSync(cut, fx.goldsrcFramesDemo({ playback: [net(0.5, 1)] }).subarray(0, 700));
  assert.deepEqual(await scanDemo(cut), { ok: false, code: "DEMO_DIRECTORY_INVALID" });
  const text = path.join(dir, "text.dem");
  fs.writeFileSync(text, "not a demo at all");
  assert.equal((await scanDemo(text)).ok, false);
  // Неизвестный тип кадра — стоп с broken.
  const odd = path.join(dir, "odd.dem");
  fs.writeFileSync(odd, fx.goldsrcFramesDemo({ playback: [net(0.5, 1, { buttons: 2 }), { type: 42, time: 1 }] }));
  const oddScan = await scanDemo(odd);
  assert.deepEqual([oddScan.broken, oddScan.buttons.length], [true, 1]);
});

// Подделка из ревью (I-1): сообщение ~1 МБ из чередующихся maps/a.bsp и maps/b.bsp,
// директория из N записей на один и тот же участок файла.
function forgedDemo(entries) {
  const message = "maps/a.bsp maps/b.bsp ".repeat(45000);
  const base = fx.goldsrcFramesDemo({ duration: 1, playback: [net(0, 0, { message }), net(1, 100)] });
  const dirOffset = base.readInt32LE(540);
  const playback = base.subarray(dirOffset + 4 + 92, dirOffset + 4 + 184);
  const dir = Buffer.alloc(4 + entries * 92);
  dir.writeInt32LE(entries, 0);
  for (let i = 0; i < entries; i++) playback.copy(dir, 4 + i * 92);
  return Buffer.concat([base.subarray(0, dirOffset), dir]);
}

test("недоверенная демка: пересекающиеся записи (16 и 1024) — отказ, смены карт и метки в пределах; ответ inspect не раздувается", async (t) => {
  const dir = tempDir(t);
  const heapBefore = process.memoryUsage().heapUsed;
  const files = [16, 1024].map((count) => {
    const file = path.join(dir, `forged${count}.dem`);
    fs.writeFileSync(file, forgedDemo(count));
    return file;
  });
  for (const file of files) {
    assert.deepEqual(await scanDemo(file), { ok: false, code: "DEMO_DIRECTORY_INVALID" }, path.basename(file));
    // Заголовок тоже не множит длительность: демка в списке, но без рендера.
    const header = headers.parseDemoFile(file);
    assert.deepEqual([header.supported, header.reason], [false, "DEMO_DIRECTORY_INVALID"]);
  }
  // Та же подделка с честной директорией: из сообщения — одна карта, не 90 тысяч.
  const single = path.join(dir, "single.dem");
  fs.writeFileSync(single, forgedDemo(1));
  assert.deepEqual((await scanDemo(single)).mapChanges.map((change) => change.map), ["a"]);
  // Тысячи кадров со сменой карты: не больше MAX_MAP_CHANGES, разбор помечен оборванным.
  const flip = path.join(dir, "flip.dem");
  const playback = [];
  for (let k = 0; k < 2000; k++) playback.push(net(k * 0.01, k, { message: `maps/${k % 2 ? "b" : "a"}.bsp` }));
  fs.writeFileSync(flip, fx.goldsrcFramesDemo({ duration: 20, playback }));
  const flipped = await scanDemo(flip);
  assert.equal(flipped.mapChanges.length, MAX_MAP_CHANGES);
  assert.equal(flipped.broken, true);

  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: path.join(dir, "jobs"), production: true });
  const added = tool.addPaths(files.concat([flip]));
  let answer = 0;
  for (const id of added.added) {
    const inspected = await tool.inspect(id);
    assert.ok(inspected.markers.length <= gsr.MAX_MARKERS, `меток ${inspected.markers.length}`);
    answer += JSON.stringify(inspected).length;
  }
  assert.ok(answer < 512 * 1024, `ответ demo-inspect ${answer} байт`);
  // Метки клипа — не больше MAX_MARKERS, сколько бы смен карт ни было.
  const picked = gsr.planSegments(Array.from({ length: 6 }, () => ({ duration: 10, map: "a" })), null).picked;
  const scans = picked.map(() => ({ mapChanges: Array.from({ length: MAX_MAP_CHANGES }, (_, k) => ({ t: k * 0.01, map: k % 2 ? "b" : "a" })) }));
  assert.equal(gsr.timelineOf(picked, scans, null).length, gsr.MAX_MARKERS);
  const grown = process.memoryUsage().heapUsed - heapBefore;
  assert.ok(grown < 64 * 1024 * 1024, `куча выросла на ${Math.round(grown / 1048576)} МБ`);
});

test("сегменты: рендерятся целиком те, что пересекают интервал, пустые пропускаются", () => {
  const segments = [{ duration: 2 }, { duration: 0 }, { duration: 3 }, { duration: 4 }];
  const all = gsr.planSegments(segments, null);
  assert.deepEqual(all.picked.map((item) => [item.index, item.start, item.end]), [[0, 0, 2], [2, 2, 5], [3, 5, 9]]);
  const mid = gsr.planSegments(segments, { from: 2.5, to: 5.5 });
  assert.deepEqual([mid.picked.map((item) => item.index), mid.offset], [[2, 3], 2]);
  assert.deepEqual(gsr.planSegments(segments, { from: 0, to: 1 }).picked.map((item) => item.index), [0]);
  assert.equal(gsr.planSegments([{ duration: 0 }], null).picked.length, 0);
});

test("привязка видео ко времени демки: вырезанное в начале сегмента и на смене карты, точно по длине видео", () => {
  const picked = gsr.planSegments([{ duration: 2 }, { duration: 3 }], null).picked;
  const scans = [{ startGap: 0.07, mapChanges: [{ t: 0, map: "c1a0", gap: 0 }] },
    { startGap: 0.02, mapChanges: [{ t: 0, map: "c1a0", gap: 0 }, { t: 1, map: "c1a0d", gap: 0.18 }] }];
  // Видео сегментов короче демки на 0,05 и 0,2 с.
  const cuts = gsr.lossCuts(picked, [1.95, 2.8], scans);
  assert.deepEqual(cuts, [{ from: 0, to: 0.05 }, { from: 2, to: 2.02 }, { from: 3, to: 3.18 }]);
  const timing = { offset: 0, cuts };
  // Граница сегментов — ровно сумма длительностей видео.
  assert.equal(Math.round(planner.materialTime(2, timing) * 1000) / 1000, 1.95);
  assert.equal(Math.round(planner.materialTime(5, timing) * 1000) / 1000, 4.75);
  // Видео не короче демки — ничего не вырезано; битая демка — всё в начале.
  assert.deepEqual(gsr.lossCuts(picked, [2, 3.01], scans), []);
  assert.deepEqual(gsr.lossCuts(picked.slice(0, 1), [1.9], [null]), [{ from: 0, to: 0.1 }]);
});

test("метки HL1: начало, сейвлоуд и смена карты на стыке, смена карты внутри демки; интервал", () => {
  const segments = [{ duration: 2, map: "c1a0" }, { duration: 3, map: "c1a0" }, { duration: 4, map: "c1a1" }];
  const picked = gsr.planSegments(segments, null).picked;
  const scans = [{ mapChanges: [{ t: 0, map: "c1a0" }] }, { mapChanges: [{ t: 0, map: "c1a0" }, { t: 1.5, map: "c1a0d" }] }, null];
  assert.deepEqual(gsr.timelineOf(picked, scans, null), [
    { t: 0, type: "map", label: "c1a0" },
    { t: 2, type: "saveload", label: "2" },
    { t: 3.5, type: "map", label: "c1a0d" },
    { t: 5, type: "map", label: "c1a1" },
  ]);
  // Интервал с середины первого сегмента: начальная метка — на начале интервала.
  assert.deepEqual(gsr.timelineOf(picked.slice(0, 1), scans, { from: 1.25, to: 2 })[0], { t: 1.25, type: "map", label: "c1a0" });
  // В видео (jobs.js): смена карты внутри сегмента встаёт на стык вырезанного.
  const cuts = [{ from: 3.5, to: 3.6 }];
  const markers = planner.mapTimeline(gsr.timelineOf(picked, scans, null), { from: 0, to: 9, speed: 1, offset: 0, cuts });
  assert.deepEqual(markers.map((marker) => marker.t), [0, 2, 3.5, 4.9]);
});

test("KeyHUD: нажатия в ASS во времени материала, с вырезанным и стыками; клавиши на месте", () => {
  const picked = gsr.planSegments([{ duration: 2 }, { duration: 3 }], null).picked;
  const scans = [{ buttons: [{ t: 0, buttons: 0 }, { t: 0.5, buttons: BUTTONS.forward | BUTTONS.duck }, { t: 1, buttons: BUTTONS.duck }] },
    { buttons: [{ t: 0, buttons: 0 }, { t: 2, buttons: BUTTONS.jump }, { t: 2.1, buttons: 0 }] }];
  const timing = { offset: 0, cuts: [{ from: 0, to: 0.05 }, { from: 2, to: 2.2 }] };
  const events = gsr.keyEvents(picked, scans, timing);
  assert.deepEqual(events.find((event) => event.buttons === (BUTTONS.forward | BUTTONS.duck)).t, 0.45);
  const ass = gsr.keyhudAss(events, { width: 1920, height: 1080, end: 4.75 });
  assert.match(ass, /PlayResX: 1920\nPlayResY: 1080/);
  const dialogue = () => ass.split("\n").filter((line) => line.startsWith("Dialogue: 1,")).map((line) => line.split(",").slice(1, 3).join("-"));
  // W: 0,45–0,95; DUCK: 0,45–1,95 (сброс на стыке); JUMP: 2-й сегмент 2,0 с -> 1,95 + 1,8 = 3,75.
  assert.deepEqual(dialogue(), ["0:00:00.45-0:00:00.95", "0:00:00.45-0:00:01.95", "0:00:03.75-0:00:03.85"]);
  for (const key of gsr.KEYS) assert.ok(ass.includes(`}${key.label}\n`), key.label);
  assert.equal(gsr.assTime(3725.456), "1:02:05.46");
  // Клавиши справа, не наезжают на край кадра.
  for (const match of ass.matchAll(/\\pos\((\d+),(\d+)\)/g)) assert.ok(Number(match[1]) < 1920 && Number(match[2]) < 1080 * 0.8);
});

test("журнал игры: сегменты, конец по «Recording stopped.», ошибки bxt-rs и движка", () => {
  assert.deepEqual(gsr.parseLogLine("Playing demo from hlsr_render/ab12cd34\\s002.dem.\r"), { kind: "playing", index: 2 });
  assert.deepEqual(gsr.parseLogLine("Playing demo from hlsr_render/ab12cd34/s010.dem."), { kind: "playing", index: 10 });
  assert.deepEqual(gsr.parseLogLine("Recording stopped."), { kind: "stopped" });
  assert.equal(gsr.parseLogLine("Playing 3 demos."), null, "очередь демок рендеру не нужна");
  const code = (line) => (gsr.parseLogLine(line) || {}).code;
  assert.equal(code("Error initializing recording: could not start ffmpeg. Make sure you have ffmpeg.exe in the Half-Life folder."), "FFMPEG_START_FAILED");
  assert.equal(code("Error during recording: broken pipe"), "RENDER_FAILED");
  assert.equal(code("Unknown command: bxt_play_folder"), "BXTRS_FAILED");
  assert.equal(code("Host_Error: Mod_NumForName: maps/c2a5.bsp not found"), "MAP_MISSING");
  // Строки движка WON при убранной карте (проверено на игре).
  assert.equal(code("Couldn't find map maps/c1a0d.bsp, server will download the map"), "MAP_MISSING");
  assert.equal(code("Warning! File maps/c1a0d.bsp missing during demo playback."), "MAP_MISSING");
  assert.equal(code("Cannot continue without model maps/c1a0d.bsp, disconnecting."), "MAP_MISSING");
  assert.equal(code("Cannot continue without model models/v_9mmhandgun.mdl, disconnecting."), "GAME_ERROR");
  assert.equal(gsr.parseLogLine("Unknown command: w300"), null, "команды из конфига игрока — не ошибка");
  assert.equal(code("Host_Error: something else"), "GAME_ERROR");
  assert.equal(gsr.parseLogLine("Recording into valve_WON/hlsr_render/x/out/s001.mp4."), null);
  assert.equal(gsr.progressSeconds("frame=1\nout_time_us=1500000\nprogress=continue\nout_time_us=2500000\n"), 2.5);
  assert.equal(gsr.progressSeconds(""), null);
});

test("job.cfg: HUD игры и «Персонализация» игрока — строки после его конфига, до запуска демок", () => {
  const base = gsr.jobConfig({ fps: 60, dir: "hlsr_render/ab12cd34" });
  assert.equal(gsr.jobConfig({ fps: 60, dir: "hlsr_render/ab12cd34", hud: true, ownHud: true }), base, "по умолчанию — как у игрока, без лишних строк");
  assert.ok(!/bxt_disable_hud|bxt_hud_game_color|crosshair/.test(base));
  const lines = (cfg) => cfg.split("\n");
  // Без HUD: HUD игры и BXT, прицел — выключены, до bxt_play_folder.
  const bare = lines(gsr.jobConfig({ fps: 60, dir: "hlsr_render/ab12cd34", hud: false }));
  for (const line of ["bxt_disable_hud 1", "bxt_hud 0", "bxt_cross 0", "crosshair 0"]) {
    assert.ok(bare.includes(line), line);
    assert.ok(bare.indexOf(line) < bare.findIndex((value) => value.startsWith("bxt_play_folder")), line);
  }
  // Стандартный HUD: значения «Персонализации» по умолчанию, без комментариев.
  const stock = lines(gsr.jobConfig({ fps: 60, dir: "hlsr_render/ab12cd34", ownHud: false }));
  for (const line of ['bxt_hud_game_color ""', 'bxt_cross "0"', 'crosshair "1"', 'default_fov "90"']) assert.ok(stock.includes(line), line);
  assert.ok(!stock.some((line) => line.startsWith("//")));
  assert.ok(!stock.includes("bxt_disable_hud 1"));
  // Оба: без HUD побеждает — его строки позже.
  const both = lines(gsr.jobConfig({ fps: 60, dir: "hlsr_render/ab12cd34", hud: false, ownHud: false }));
  assert.ok(both.indexOf("crosshair 0") > both.indexOf('crosshair "1"'));
});

test("job.cfg и командная строка: пути без «-» и «+», всё через +exec; Injector и полный путь hl.exe", () => {
  const cfg = gsr.jobConfig({ fps: 60, dir: "hlsr_render/ab12cd34" });
  assert.match(cfg, /^bxt_cap_fps 60$/m);
  assert.match(cfg, /^bxt_cap_skip_non_gameplay_frames 1$/m);
  assert.match(cfg, /^bxt_play_folder hlsr_render\/ab12cd34$/m);
  assert.match(cfg, /^bxt_cap_separate_start valve_WON\/hlsr_render\/ab12cd34\/out$/m);
  assert.match(cfg, /-progress valve_WON\/hlsr_render\/ab12cd34\/progress\.txt/);
  assert.ok(/^_bxt_cap_override_ffmpeg_args "[^"\s]+(?: [^"\s]+)*"$/m.test(cfg), "аргументы FFmpeg без пробелов в путях");
  assert.ok(!/[-+]/.test(gsr.RENDER_DIR));
  const args = gsr.gameArguments({ width: 1920, height: 1080, dir: "hlsr_render/ab12cd34" });
  assert.deepEqual(args.slice(-3), ["-condebug", "+exec", "hlsr_render/ab12cd34/job.cfg"]);
  for (const flag of ["-windowed", "-game", "valve_WON", "-steam"]) assert.ok(args.includes(flag), flag);
  assert.ok(!gsr.resetArguments().includes("+exec"), "сброс mute — без демок и bxt-rs");
});

test("окружение игры: FFmpeg инструмента первым в PATH, Discord RPC и vsync BXT выключены, лишнее срезает помощник", () => {
  const env = goldsrc.gameEnv({ Path: "C:\\Windows", BXT_DISABLE_VSYNC: "0", HLSR_PRESENCE_PIPE: "x", SLINT_BACKEND: "y", SystemRoot: "C:\\Windows" }, "D:\\tools\\ffmpeg");
  assert.equal(env.PATH, `D:\\tools\\ffmpeg${path.delimiter}C:\\Windows`);
  assert.equal(Object.keys(env).filter((key) => /^path$/i.test(key)).length, 1, "один PATH, без дубля Path");
  assert.equal(env.BXT_DISABLE_DISCORD_RPC, "1");
  assert.equal(env.BXT_DISABLE_VSYNC, "1");
  assert.deepEqual(Object.keys(childEnv(env)).sort(), ["BXT_DISABLE_DISCORD_RPC", "BXT_DISABLE_VSYNC", "PATH", "SystemRoot"]);
});

test("кодирование: KeyHUD — фильтр ass по времени материала; оверлей только из папки задания", (t) => {
  const plan = planner.planExport(Object.assign({}, planner.DEFAULTS, { width: 1920, range: { from: 0, to: 10 } }), { encoder: "libx264", outputDuration: 10 });
  const args = planner.encodeArgs({ material: { video: "m.mkv", audio: "m.mkv", overlay: "keyhud.ass" }, trim: { start: 2.5, duration: 10 }, plan, output: "o.mp4", pass: 0 });
  const filter = args[args.indexOf("-filter:v") + 1];
  assert.ok(filter.startsWith("setpts=PTS-STARTPTS+2.5/TB,ass=keyhud.ass,setpts=(PTS-STARTPTS)/1,"), filter);
  assert.equal(args.filter((arg) => arg === "m.mkv").length, 2, "звук из того же файла — вторым входом");
  const plain = planner.encodeArgs({ material: { video: "m.mkv" }, trim: { start: 0, duration: 10 }, plan, output: "o.mp4", pass: 0 });
  assert.ok(!plain[plain.indexOf("-filter:v") + 1].includes("ass="));
  // jobs.js: оверлей — имя файла в папке задания, не путь.
  const dir = tempDir(t);
  const material = path.join(dir, "m.mkv");
  fs.writeFileSync(material, "x");
  fs.writeFileSync(path.join(dir, "keyhud.ass"), "x");
  const runJob = (overlay, id) => jobs.runJob({ id, kind: "export", item: { segments: [] }, settings: Object.assign({}, planner.DEFAULTS, { range: { from: 0, to: 1 } }),
    plan, root: path.join(dir, "jobs"), adapter: { render: async ({ jobDir }) => {
      fs.copyFileSync(material, path.join(jobDir, "m.mkv"));
      if (overlay === "keyhud.ass") fs.copyFileSync(path.join(dir, "keyhud.ass"), path.join(jobDir, overlay));
      return { video: path.join(jobDir, "m.mkv"), audio: null, overlay };
    } } });
  return Promise.all([
    assert.rejects(runJob("..\\..\\x.ass", "j1"), { code: "RENDER_FAILED" }),
    assert.rejects(runJob("missing.ass", "j2"), { code: "RENDER_FAILED" }),
  ]);
});

test("KeyHUD на настоящем FFmpeg: libass рисует нажатую клавишу на своих кадрах после обрезки", (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const dir = tempDir(t);
  const material = path.join(dir, "material.mkv");
  spawnSync(ff.ffmpeg, ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black:size=640x360:rate=30:duration=4", "-c:v", "libx264", "-preset", "ultrafast", material]);
  // W нажата с 1,0 до 2,0 с материала.
  fs.writeFileSync(path.join(dir, "keyhud.ass"), gsr.keyhudAss([{ t: 0, buttons: 0 }, { t: 1, buttons: BUTTONS.forward }, { t: 2, buttons: 0 }], { width: 640, height: 360, end: 4 }));
  const plan = planner.planExport({ preset: "preview", width: 640, height: 360, fps: 30, speed: 1, audio: false, keyhud: true, range: { from: 0.5, to: 3.5 } },
    { encoder: "libx264", outputDuration: 3 });
  plan.width = 640;
  plan.height = 360;
  const args = planner.encodeArgs({ material: { video: material, overlay: "keyhud.ass" }, trim: { start: 0.5, duration: 3 }, plan, output: path.join(dir, "out.mp4"), pass: 0 });
  const run = spawnSync(ff.ffmpeg, args, { cwd: dir, encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr);
  // Клавиша W: центр её прямоугольника в кадре 640×360.
  const w = gsr.keyhudAss([], { width: 640, height: 360, end: 1 }).split("\n").find((line) => line.endsWith("}W"));
  const [x, y] = /\\pos\((\d+),(\d+)\)/.exec(w).slice(1).map(Number);
  const pixel = (at) => {
    const out = spawnSync(ff.ffmpeg, ["-hide_banner", "-loglevel", "error", "-ss", String(at), "-i", path.join(dir, "out.mp4"), "-frames:v", "1",
      // 2×2: у yuv420p меньше нельзя.
      "-vf", `crop=2:2:${x - 6}:${y - 6}`, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], { maxBuffer: 1024 });
    return [...out.stdout].slice(0, 3);
  };
  // Выход 0,2 с = материал 0,7 (не нажата), выход 1,0 = материал 1,5 (нажата: оранжевая).
  const idle = pixel(0.2);
  const pressed = pixel(1.0);
  assert.ok(pressed[0] > 150 && pressed[0] > pressed[2] + 80, `нажата: ${pressed}`);
  assert.ok(idle[0] < 80, `не нажата: ${idle}`);
});

test("jobs.js: предупреждения адаптера (KEYHUD_PARTIAL) попадают в итог, мусор — нет", async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const dir = tempDir(t);
  const material = path.join(dir, "m.mkv");
  spawnSync(ff.ffmpeg, ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black:size=640x360:rate=30:duration=2", "-c:v", "libx264", "-preset", "ultrafast", material]);
  const settings = { preset: "preview", width: 640, height: 360, fps: 30, speed: 1, audio: false, keyhud: false, range: { from: 0, to: 1 } };
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 1 });
  const result = await jobs.runJob({ id: "w1", kind: "preview", item: { segments: [] }, settings, plan, root: path.join(dir, "jobs"), ffmpegPath: ff.ffmpeg, ffprobePath: ff.ffprobe,
    adapter: { render: async ({ jobDir }) => {
      fs.copyFileSync(material, path.join(jobDir, "m.mkv"));
      return { video: path.join(jobDir, "m.mkv"), audio: null, offset: 0, warnings: ["KEYHUD_PARTIAL", "bad code", 5] };
    } } });
  assert.deepEqual(result.warnings, ["KEYHUD_PARTIAL"]);
});

// ---------- адаптер целиком: подставная игра ----------

function argv(line) {
  const out = [];
  for (const match of line.matchAll(/"((?:[^"\\]|\\.)*)"|(\S+)/g)) out.push(match[1] !== undefined ? match[1] : match[2]);
  return out;
}

function fakeRegistry(initial) {
  const keys = new Map(initial ? [[goldsrc.SPEC.registryKey, Object.assign({}, initial)]] : []);
  return {
    keys,
    exists: (key) => keys.has(key),
    exportKey: (key, file) => fs.writeFileSync(file, JSON.stringify(keys.get(key))),
    deleteKey: (key) => keys.delete(key),
    importFile: (file) => keys.set(goldsrc.SPEC.registryKey, JSON.parse(fs.readFileSync(file, "utf8"))),
  };
}

// Подставная игра за помощником: читает job.cfg из +exec, пишет в qconsole.log
// строки bxt-rs и движка, видео сегментов в out/ и прогресс; как bxt-rs —
// «Recording stopped.» после каждого сегмента. Глушит «звук» и снимает mute сбросом.
class FakeHost extends EventEmitter {
  constructor(game) {
    super();
    this.game = game;
    this.exitCode = null;
    this.closed = false;
    this.pending = null;
    this.stopFlag = false;
  }

  async start(request) {
    this.request = request;
    this.game.launches.push(request);
    if (request.resetOnly) {
      // Помощник в режиме resetOnly: один беззвучный запуск, снять mute, «stopped», выход; stop() только ждёт.
      this.running = Promise.resolve();
      this.emit("event", { event: "started", pid: 0 });
      this.stopping = new Promise((resolve) => setImmediate(() => {
        this.emit("event", { event: "audio-reset", ok: true });
        this.pending = false;
        this.emit("event", { event: "stopped", code: 0, pending: false });
        this.closed = true;
        resolve();
      }));
      return 0;
    }
    this.running = this.simulate(argv(request.commandLine), request.cwd).catch((error) => this.game.errors.push(error));
    this.emit("event", { event: "started", pid: 4242 });
    this.emit("event", { event: "audio", action: "muted", pid: 4243, device: 0, was: false });
    return 4242;
  }

  async simulate(args, hl) {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const exec = args[args.indexOf("+exec") + 1];
    const dir = path.dirname(exec);
    const won = path.join(hl, "valve_WON");
    const jobDir = path.join(won, ...dir.split("/"));
    this.game.cfg = fs.readFileSync(path.join(won, ...exec.split("/")), "utf8");
    this.game.injected = fs.readdirSync(path.dirname(args[0])).sort();
    const say = (line) => fs.appendFileSync(path.join(hl, "qconsole.log"), `${line}\r\n`);
    const hang = async () => {
      while (!this.stopFlag) await sleep(20);
    };
    // noBxt: bxt-rs не встал (ни строки в bxt-rs.log, ни демок); silent: встал, но игра молчит.
    if (!this.game.noBxt) fs.appendFileSync(path.join(hl, "bxt-rs.log"), "found the WON engine inside hl.exe\n");
    if (this.game.noBxt || this.game.silent) return hang();
    if (this.game.unknown) {
      say("Unknown command: bxt_play_folder");
      return;
    }
    const demos = fs.readdirSync(jobDir).filter((name) => name.endsWith(".dem")).sort();
    say(`Playing ${demos.length} demos.`);
    for (const name of demos) {
      say(`Playing demo from ${dir}/${name}.`);
      fs.mkdirSync(path.join(jobDir, "out"), { recursive: true });
      fs.writeFileSync(path.join(jobDir, "out", name.replace(/\.dem$/, ".mp4")), "video");
      fs.writeFileSync(path.join(jobDir, "progress.txt"), "out_time_us=500000\nprogress=continue\n");
      // afterFirst(host): что случится посреди первой демки (выход, чужой hl.exe…), дальше игра молчит.
      if (this.game.afterFirst) {
        this.game.afterFirst(this, say);
        return hang();
      }
      await sleep(this.game.segmentMs);
      if (this.stopFlag) return;
      say("Recording stopped.");
    }
  }

  get helperPid() {
    return null;
  }

  stop(reset) {
    if (this.stopping) return this.stopping;
    this.stopFlag = true;
    this.game.resets.push(reset !== false);
    this.stopping = this.running.then(() => {
      if (reset !== false) this.emit("event", { event: "audio-reset", ok: true });
      this.pending = reset === false;
      this.emit("event", { event: "stopped", code: 0, pending: this.pending });
    });
    return this.stopping;
  }

  // Выход лаунчера: помощник сам закрывает игру; для рендера он пропал.
  detach() {
    this.stopFlag = true;
    this.game.detached = true;
    this.closed = true;
  }

  kill() {
    this.stopFlag = true;
  }
}

function setupAdapter(t, options) {
  const opts = options || {};
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  const hl = path.join(install, "Half-Life");
  const bxtDir = path.join(install, "Bunnymod XT");
  fs.mkdirSync(path.join(hl, "valve_WON"), { recursive: true });
  fs.mkdirSync(bxtDir, { recursive: true });
  fs.writeFileSync(path.join(hl, "hl.exe"), "");
  for (const name of ["Injector.exe", "BunnymodXT.dll", "7za.exe", "version"]) fs.writeFileSync(path.join(bxtDir, name), name);
  fs.writeFileSync(path.join(hl, "qconsole.log"), "player log\r\n");
  const [one, two] = runDemos(dir);
  const item = library.describe({ kind: "segments", files: [one, two], name: "run" });
  const dll = path.join(dir, "bxt_rs.dll");
  fs.writeFileSync(dll, "bxt-rs");
  const sha256 = crypto.createHash("sha256").update("bxt-rs").digest("hex");
  const jobsRoot = path.join(dir, "jobs");
  const jobDir = path.join(jobsRoot, "abcdef0123456789");
  fs.mkdirSync(jobDir, { recursive: true });
  const reg = fakeRegistry({ ScreenWidth: 2560 });
  const game = Object.assign({ launches: [], errors: [], resets: [], segmentMs: 40, unknown: opts.unknown }, opts.game);
  const store = fx.memoryStore();
  const ran = [];
  const adapter = new goldsrc.Hl1Adapter(Object.assign({
    platform: "win32",
    jobsRoot,
    store,
    registry: reg,
    script: "render-host.ps1",
    installPath: () => install,
    gameRunning: () => false,
    steamStatus: () => ({ ok: true }),
    inspector: { findByImageName: () => [], samePath: (a, b) => a === b },
    freeSpace: () => 1e15,
    hostFactory: (hostOptions) => {
      game.hostEnv = hostOptions.env;
      return new FakeHost(game);
    },
    bxtrs: () => ({ path: dll, sha256: opts.sha256 || sha256 }),
    // Видео сегментов: 1,95 и 2,8 с (демки 2 и 3 с).
    ffprobeFn: async (exe, file) => ({ streams: [{ codec_type: "video", duration: path.basename(file) === "s001.mp4" ? "1.950000" : "2.800000" }] }),
    runFn: async (exe, args) => {
      ran.push(args);
      fs.writeFileSync(args[args.length - 1], "material");
    },
  }, opts.adapter || {}));
  const request = Object.assign({ item, range: { from: 0, to: item.duration }, fps: 60, width: 1280, height: 720, audio: true, keyhud: true,
    jobDir, ffmpegPath: path.join(dir, "ffmpeg", "ffmpeg.exe"), ffprobePath: path.join(dir, "ffmpeg", "ffprobe.exe"), onProgress: () => {} }, opts.request || {});
  return { dir, install, hl, bxtDir, item, jobsRoot, jobDir, reg, game, store, adapter, request, ran };
}

const listDir = (dir) => (fs.existsSync(dir) ? fs.readdirSync(dir).sort() : null);

test("адаптер HL1: возможности и отказы — не HL1, Linux, нет игры, чужой протокол, пустой ран, нет bxt-rs", (t) => {
  const { adapter, item, install } = setupAdapter(t);
  assert.deepEqual(adapter.canRender(item), { ok: true });
  assert.deepEqual(adapter.capabilities, { maxHeight: 1080, fps: [30, 60, 120], keyhud: true, audio: true, hud: true, ownHud: true });
  assert.equal(adapter.canRender(Object.assign({}, item, { game: "ghosting" })).code, "DEMO_UNSUPPORTED");
  assert.equal(new goldsrc.Hl1Adapter({ platform: "linux", script: "x", installPath: () => install }).canRender(item).code, "RENDER_NOT_READY");
  assert.equal(new goldsrc.Hl1Adapter({ platform: "win32", script: "x", installPath: () => null }).canRender(item).code, "GAME_NOT_INSTALLED");
  const steamDemo = Object.assign({}, item, { segments: item.segments.map((segment) => Object.assign({}, segment, { netProtocol: 48 })) });
  assert.equal(adapter.canRender(steamDemo).code, "DEMO_PROTOCOL_UNSUPPORTED");
  assert.equal(adapter.canRender(Object.assign({}, item, { segments: item.segments.map((segment) => Object.assign({}, segment, { duration: 0 })) })).code, "DEMO_EMPTY");
  const noTool = new goldsrc.Hl1Adapter({ platform: "win32", script: "x", installPath: () => install, bxtrs: () => null });
  assert.equal(noTool.canRender(item).code, "BXTRS_MISSING");
});

test("адаптер HL1: рендер сегментов, материал и KeyHUD; bxt_rs.dll только в папке задания; игроку всё как было", { timeout: 60000 }, async (t) => {
  const { adapter, request, game, reg, hl, bxtDir, jobDir, jobsRoot, store, ran, install } = setupAdapter(t);
  game.segmentMs = 400;
  const before = { bxt: listDir(bxtDir), qconsole: fs.readFileSync(path.join(hl, "qconsole.log"), "utf8") };
  const progress = [];
  request.onProgress = (info) => progress.push(info);
  let activeDuring = false;
  const pending = adapter.render(request);
  setTimeout(() => { activeDuring = adapter.isActive(); }, 20);
  const material = await pending;
  assert.deepEqual(game.errors, []);
  assert.equal(activeDuring, true);
  assert.equal(adapter.isActive(), false);

  // Injector из своей папки: только Injector, BXT и bxt-rs; полный путь hl.exe; посредник и сброс mute — самой игрой.
  assert.deepEqual(game.injected, ["BunnymodXT.dll", "Injector.exe", "bxt_rs.dll"]);
  const start = game.launches[0];
  const args = argv(start.commandLine);
  assert.equal(args[0], path.join(jobDir, "inject", "Injector.exe"));
  assert.equal(args[1], path.join(hl, "hl.exe"));
  assert.equal(start.cwd, hl);
  assert.deepEqual([start.followJob, start.mute, start.resetExe], [true, true, path.join(hl, "hl.exe")]);
  assert.equal(argv(start.resetCommandLine)[0], path.join(hl, "hl.exe"));
  assert.equal(game.hostEnv.BXT_DISABLE_DISCORD_RPC, "1");
  assert.ok(game.hostEnv.PATH.startsWith(path.dirname(request.ffmpegPath)));
  assert.match(game.cfg, /bxt_play_folder hlsr_render\/abcdef01/);

  // Склейка сегментов: concat с длительностью каждого видео.
  const concat = ran[0];
  assert.deepEqual(concat.slice(concat.indexOf("-f"), concat.indexOf("-f") + 2), ["-f", "concat"]);
  assert.equal(material.video, path.join(jobDir, "material.mkv"));
  assert.equal(material.audio, material.video);
  assert.equal(material.offset, 0);
  // Сегмент 2 с оружием: почти вся потеря (0,2 с) — на смене карты.
  assert.deepEqual(material.cuts, [{ from: 0, to: 0.05 }, { from: 2, to: 2.001 }, { from: 3, to: 3.199 }]);
  assert.deepEqual(material.timeline, [{ t: 0, type: "map", label: "c1a0" }, { t: 2, type: "saveload", label: "2" }, { t: 3, type: "map", label: "c1a0d" }]);
  assert.equal(material.overlay, "keyhud.ass");
  const ass = fs.readFileSync(path.join(jobDir, "keyhud.ass"), "utf8");
  assert.ok(ass.includes("Dialogue: 1,0:00:00.45,0:00:00.95"), "W 0,5–1,0 демки -> 0,45–0,95 видео");
  assert.ok(progress.some((info) => info.segment && info.segment.index === 2 && info.segment.count === 2));
  assert.ok(progress.every((info, k) => k === 0 || info.fraction >= progress[k - 1].fraction), "доля не убывает");

  // Возвращено: реестр, журналы, папка «Bunnymod XT», папка рендера, маркер; mute снят сбросом.
  assert.deepEqual(reg.keys.get(goldsrc.SPEC.registryKey), { ScreenWidth: 2560 });
  assert.equal(fs.readFileSync(path.join(hl, "qconsole.log"), "utf8"), before.qconsole);
  assert.equal(fs.existsSync(path.join(hl, "bxt-rs.log")), false);
  assert.deepEqual(listDir(bxtDir), before.bxt);
  assert.equal(fs.existsSync(path.join(hl, "valve_WON", "hlsr_render")), false);
  assert.deepEqual(listDir(jobsRoot), ["abcdef0123456789"]);
  assert.deepEqual(game.resets, [true]);
  assert.notEqual(store.get(goldsrc.SPEC.unmutePending), true);
  assert.equal(fs.existsSync(path.join(install, "Bunnymod XT", "bxt_rs.dll")), false);
});

test("адаптер HL1: bxt-rs не встал — BXTRS_FAILED; подменённая DLL — BXTRS_BROKEN до запуска игры", { timeout: 60000 }, async (t) => {
  const failed = setupAdapter(t, { unknown: true });
  await assert.rejects(failed.adapter.render(failed.request), { code: "BXTRS_FAILED" });
  assert.equal(fs.existsSync(path.join(failed.hl, "valve_WON", "hlsr_render")), false);
  assert.deepEqual(failed.reg.keys.get(goldsrc.SPEC.registryKey), { ScreenWidth: 2560 });
  assert.equal(fs.existsSync(path.join(failed.hl, "bxt-rs.log")), false);

  const broken = setupAdapter(t, { sha256: "0".repeat(64) });
  await assert.rejects(broken.adapter.render(broken.request), { code: "BXTRS_BROKEN" });
  assert.equal(broken.game.launches.length, 0);

  // Демка оборвалась (а «Recording stopped.» всё равно пришёл): видео сегмента много короче демки.
  const cut = setupAdapter(t, { adapter: { ffprobeFn: async () => ({ streams: [{ codec_type: "video", duration: "0.500000" }] }) } });
  await assert.rejects(cut.adapter.render(cut.request), { code: "RENDER_FAILED", message: /Сегмент 1/ });
  assert.equal(fs.existsSync(path.join(cut.hl, "valve_WON", "hlsr_render")), false);
});

test("адаптер HL1: отмена посреди рендера — CANCELLED, игра закрыта, реестр, журналы и папки возвращены", { timeout: 60000 }, async (t) => {
  const { adapter, request, game, reg, hl } = setupAdapter(t);
  game.segmentMs = 2000;
  const controller = new AbortController();
  request.signal = controller.signal;
  const pending = adapter.render(request);
  setTimeout(() => controller.abort(), 300);
  await assert.rejects(pending, { code: "CANCELLED" });
  assert.equal(adapter.isActive(), false);
  assert.deepEqual(reg.keys.get(goldsrc.SPEC.registryKey), { ScreenWidth: 2560 });
  assert.equal(fs.readFileSync(path.join(hl, "qconsole.log"), "utf8"), "player log\r\n");
  assert.equal(fs.existsSync(path.join(hl, "valve_WON", "hlsr_render")), false);
});

// Игроку всё как было: реестр, журналы, папка рендера, маркер и копии журналов.
function assertRestored(s, label) {
  assert.deepEqual(s.reg.keys.get(goldsrc.SPEC.registryKey), { ScreenWidth: 2560 }, label);
  assert.equal(fs.readFileSync(path.join(s.hl, "qconsole.log"), "utf8"), "player log\r\n", label);
  assert.equal(fs.existsSync(path.join(s.hl, "bxt-rs.log")), false, label);
  assert.equal(fs.existsSync(path.join(s.hl, "valve_WON", "hlsr_render")), false, label);
  assert.equal(gr.readMarker(s.jobsRoot, goldsrc.SPEC), null, label);
  assert.deepEqual(fs.readdirSync(s.jobsRoot).filter((name) => name.startsWith("hl1-render.")), [], label);
}

test("адаптер HL1: предусловия — другой hl.exe, та же игра, Steam, место, ffmpeg.exe в папке игры; игра не запускается", { timeout: 60000 }, async (t) => {
  const other = { findByImageName: () => [{ pid: 77, executablePath: "D:\\Steam\\steamapps\\common\\Half-Life\\hl.exe" }], samePath: (a, b) => a === b };
  for (const [code, options] of [
    ["GOLDSRC_RUNNING", { adapter: { inspector: other } }],
    ["STEAM_NOT_RUNNING", { adapter: { steamStatus: () => ({ ok: false, code: "STEAM_NOT_RUNNING" }) } }],
    ["STEAM_ELEVATED", { adapter: { steamStatus: () => ({ ok: false, code: "STEAM_ELEVATED" }) } }],
    ["NO_SPACE", { adapter: { freeSpace: () => 1024 } }],
    ["GAME_RUNNING", { adapter: { gameRunning: (gameId) => gameId === "hl1-won" } }],
  ]) {
    const s = setupAdapter(t, options);
    await assert.rejects(s.adapter.render(s.request), { code }, code);
    assert.equal(s.game.launches.length, 0, code);
    assertRestored(s, code);
  }
  // Та же WON-игра запущена не из лаунчера — GAME_RUNNING (путь совпал), а не «другая игра».
  const same = setupAdapter(t);
  same.adapter.inspector = { findByImageName: () => [{ pid: 78, executablePath: path.join(same.hl, "hl.exe") }], samePath: (a, b) => a === b };
  await assert.rejects(same.adapter.render(same.request), { code: "GAME_RUNNING" });
  // Чужой ffmpeg.exe рядом с hl.exe перебил бы FFmpeg инструмента.
  const ff = setupAdapter(t);
  fs.writeFileSync(path.join(ff.hl, "ffmpeg.exe"), "");
  await assert.rejects(ff.adapter.render(ff.request), { code: "FFMPEG_IN_GAME_DIR" });
  assert.equal(ff.game.launches.length, 0);
});

test("адаптер HL1: сегмент пропал или заменён после добавления — код с номером и именем сегмента", { timeout: 60000 }, async (t) => {
  const missing = setupAdapter(t);
  fs.rmSync(missing.item.segments[1].path);
  await assert.rejects(missing.adapter.render(missing.request), { code: "SEGMENT_MISSING", details: { segment: 2, name: "run_2.dem" } });
  // Дописали байты (другой размер) или подложили не-демку того же размера.
  for (const replace of [(file) => fs.appendFileSync(file, "x"), (file) => fs.writeFileSync(file, Buffer.alloc(fs.statSync(file).size))]) {
    const s = setupAdapter(t);
    replace(s.item.segments[0].path);
    await assert.rejects(s.adapter.render(s.request), { code: "SEGMENT_REPLACED", details: { segment: 1, name: "run_1.dem" } });
    assert.equal(s.game.launches.length, 0);
    assertRestored(s);
  }
});

test("адаптер HL1: чужой hl.exe появился посреди рендера — GOLDSRC_RUNNING, реестр и журналы возвращены сразу", { timeout: 60000 }, async (t) => {
  const s = setupAdapter(t, { game: { afterFirst: (host) => host.emit("event", { event: "foreign", pid: 991 }) } });
  await assert.rejects(s.adapter.render(s.request), { code: "GOLDSRC_RUNNING" });
  assert.equal(s.game.launches[0].watchImage, "hl");
  assertRestored(s);
  // Без перезапуска-сброса mute: WON hl.exe рядом с чужим ждал бы мьютекс; mute снимет следующий рендер.
  assert.deepEqual(s.game.resets, [false]);
  assert.equal(s.store.get(goldsrc.SPEC.unmutePending), true);
});

test("адаптер HL1: таймауты и выходы — bxt-rs не встал, игра молчит, застой, задание опустело, помощник вышел", { timeout: 60000 }, async (t) => {
  const fast = { firstTimeoutMs: 300, stallTimeoutMs: 300 };
  for (const [code, game, label] of [
    ["BXTRS_FAILED", { noBxt: true }, "нет строки bxt-rs"],
    ["RENDER_TIMEOUT", { silent: true }, "первой демки нет"],
    ["RENDER_TIMEOUT", { afterFirst: () => {} }, "застой"],
    ["GAME_EXITED", { afterFirst: (host) => { host.exitCode = 0; } }, "задание опустело"],
    ["RENDER_HOST_FAILED", { afterFirst: (host) => { host.closed = true; } }, "помощник вышел"],
    ["MAP_MISSING", { afterFirst: (host, say) => say("Couldn't find map maps/c1a0d.bsp, server will download the map") }, "нет карты"],
  ]) {
    const s = setupAdapter(t, { game, adapter: fast });
    const error = await s.adapter.render(s.request).then(() => null, (failure) => failure);
    assert.equal(error && error.code, code, label);
    // Код выхода Injector — не код игры: в подробности он не идёт.
    if (code === "GAME_EXITED") assert.equal(error.details, undefined);
    assert.equal(s.adapter.isActive(), false, label);
    assertRestored(s, label);
  }
});

test("адаптер HL1: интервал со второго сегмента — offset ≠ 0, в игру только он, первая метка — карта на начале интервала", { timeout: 60000 }, async (t) => {
  // Сегмент 2 (2–5 с) сменил карту на c1a0d в 3 с; клип с 3,5 с.
  const s = setupAdapter(t, { request: { range: { from: 3.5, to: 5 } } });
  const material = await s.adapter.render(s.request);
  assert.equal(material.offset, 2);
  assert.deepEqual(material.timeline[0], { t: 3.5, type: "map", label: "c1a0d" });
  assert.equal(material.timeline.length, 1);
  const list = fs.readFileSync(path.join(s.jobDir, "segments.txt"), "utf8");
  assert.deepEqual([...list.matchAll(/^file '([^']+)'/gm)].map((match) => path.basename(match[1])), ["s001.mp4"]);
  assertRestored(s);
});

test("адаптер HL1: KeyHUD без данных в части демок — предупреждение KEYHUD_PARTIAL в итоге; concat обрезает звук по видео", { timeout: 60000 }, async (t) => {
  const s = setupAdapter(t, { adapter: { scanFn: async (file, options) => (path.basename(file) === "run_2.dem" ? { ok: false, code: "DEMO_DIRECTORY_INVALID" } : scanDemo(file, options)) } });
  const material = await s.adapter.render(s.request);
  assert.deepEqual(material.warnings, ["KEYHUD_PARTIAL"]);
  // Битая вторая демка: всё вырезанное — в начале сегмента (метки — по плейлисту).
  assert.deepEqual(material.timeline.map((marker) => marker.type), ["map", "saveload"]);
  const list = fs.readFileSync(path.join(s.jobDir, "segments.txt"), "utf8");
  assert.match(list, /s001\.mp4'\nduration 1\.950000\noutpoint 1\.950000\n/);
  const plain = setupAdapter(t, { request: { keyhud: false } });
  assert.deepEqual((await plain.adapter.render(plain.request)).warnings, []);
  assert.equal(goldsrc.concatList(["a'b.mp4"], [2]), "file 'a'\\''b.mp4'\nduration 2.000000\noutpoint 2.000000\n");
});

test("выход лаунчера посреди рендера HL1: disposeSync — реестр, журналы и папка возвращены, mute помечен к снятию", { timeout: 60000 }, async (t) => {
  const s = setupAdapter(t);
  s.game.segmentMs = 1500;
  const pending = s.adapter.render(s.request).then(() => null, (failure) => failure);
  for (const until = Date.now() + 10000; !s.game.launches.length && Date.now() < until;) await new Promise((resolve) => setTimeout(resolve, 20));
  await new Promise((resolve) => setTimeout(resolve, 200));
  s.adapter.disposeSync();
  assert.equal(s.game.detached, true);
  assertRestored(s);
  assert.equal(s.store.get(goldsrc.SPEC.unmutePending), true, "mute игры снимет следующий рендер");
  assert.equal((await pending).code, "RENDER_HOST_FAILED");
  assertRestored(s);
});

test("сбой посреди рендера HL1: реестр и журналы игрока возвращаются при старте по маркеру, mute помечается к снятию", (t) => {
  const root = tempDir(t);
  const reg = fakeRegistry({ ScreenWidth: 1920, ScreenWindowed: 0 });
  // Игра — не в корне заданий: его папки чистит jobs.cleanupStale.
  const hl = path.join(tempDir(t), "game", "Half-Life");
  fs.mkdirSync(hl, { recursive: true });
  fs.writeFileSync(path.join(hl, "qconsole.log"), "player log\r\n");
  const store = fx.memoryStore();
  const adapter = new goldsrc.Hl1Adapter({ jobsRoot: root, registry: reg, store, script: "x" });
  // Копии журналов — рядом с маркером (чистка папок заданий их не тронет).
  const logs = adapter.saveLogs(hl, path.join(root, "abcdef0123456789"));
  gr.snapshotRegistry(reg, root, goldsrc.SPEC, { muted: true, logs: logs.map(({ file, backup }) => ({ file, backup })) });
  assert.ok(fs.existsSync(path.join(root, "hl1-render.restore.json")));
  reg.keys.set(goldsrc.SPEC.registryKey, { ScreenWidth: 800, ScreenWindowed: 1 });
  fs.appendFileSync(path.join(hl, "qconsole.log"), "Playing demo from hlsr_render/abcdef01/s001.dem.\r\n");
  fs.writeFileSync(path.join(hl, "bxt-rs.log"), "found the WON engine\n");
  new DemoTool({ store, jobsRoot: root, hl1Adapter: adapter });
  assert.deepEqual(reg.keys.get(goldsrc.SPEC.registryKey), { ScreenWidth: 1920, ScreenWindowed: 0 });
  assert.equal(store.get("hl1UnmutePending"), true);
  assert.equal(gr.readMarker(root, goldsrc.SPEC), null);
  assert.equal(fs.readFileSync(path.join(hl, "qconsole.log"), "utf8"), "player log\r\n");
  assert.equal(fs.existsSync(path.join(hl, "bxt-rs.log")), false);
  assert.deepEqual(fs.readdirSync(root).filter((name) => /\.orig$/.test(name)), []);
  // Маркер с чужими путями: файлы вне папки Half-Life и копии не из корня заданий не трогаются.
  const otherDir = tempDir(t);
  const foreign = path.join(otherDir, "qconsole.log");
  fs.writeFileSync(foreign, "keep");
  fs.writeFileSync(path.join(otherDir, "hl1-render.qconsole.log.orig"), "not ours");
  adapter.restoreLogs([{ file: foreign, backup: null }, { file: path.join(hl, "qconsole.log"), backup: path.join(otherDir, "hl1-render.qconsole.log.orig") }]);
  assert.equal(fs.readFileSync(foreign, "utf8"), "keep");
  assert.equal(fs.readFileSync(path.join(hl, "qconsole.log"), "utf8"), "player log\r\n");

  // Реестр не вернулся (снимок .reg пропал): маркер остаётся, но журналы — только один раз.
  const root2 = tempDir(t);
  fs.rmSync(path.join(hl, "qconsole.log"));
  const again = new goldsrc.Hl1Adapter({ jobsRoot: root2, registry: reg, store, script: "x" });
  const logs2 = again.saveLogs(hl, path.join(root2, "job"));
  gr.snapshotRegistry(reg, root2, goldsrc.SPEC, { muted: false, logs: logs2.map(({ file, backup }) => ({ file, backup })) });
  fs.rmSync(path.join(root2, goldsrc.SPEC.regFile));
  fs.writeFileSync(path.join(hl, "qconsole.log"), "render output\r\n");
  again.recover();
  assert.equal(fs.existsSync(path.join(hl, "qconsole.log")), false, "до рендера журнала не было");
  assert.ok(gr.readMarker(root2, goldsrc.SPEC), "маркер ждёт реестра");
  assert.equal(gr.readMarker(root2, goldsrc.SPEC).logs, undefined);
  // Игрок после сбоя снова поиграл: следующий старт его журнал не трогает.
  fs.writeFileSync(path.join(hl, "qconsole.log"), "new player log\r\n");
  again.recover();
  assert.equal(fs.readFileSync(path.join(hl, "qconsole.log"), "utf8"), "new player log\r\n");
});

test("конец рендера HL1, реестр не вернулся: журналы возвращены один раз, следующий старт журнал игрока не трогает", (t) => {
  // Проба ревьюера (probe-hl1-logs.js): reg import падает один раз в finish().
  const base = tempDir(t);
  const jobsRoot = path.join(base, "jobs");
  fs.mkdirSync(jobsRoot);
  const hl = path.join(base, "game", "Half-Life");
  fs.mkdirSync(hl, { recursive: true });
  const keys = new Map([[goldsrc.SPEC.registryKey, { ScreenWidth: 1920 }]]);
  let failImport = 1;
  const reg = {
    exists: (k) => keys.has(k),
    exportKey: (k, f) => fs.writeFileSync(f, JSON.stringify(keys.get(k))),
    deleteKey: (k) => keys.delete(k),
    importFile: (f) => {
      if (failImport-- > 0) throw new Error("reg import timeout");
      keys.set(goldsrc.SPEC.registryKey, JSON.parse(fs.readFileSync(f, "utf8")));
    },
  };
  const adapter = new goldsrc.Hl1Adapter({ jobsRoot, registry: reg, store: fx.memoryStore(), script: "x" });
  const active = { host: null, pump: null, modJobDir: null, registry: false, muted: false, pendingBefore: false };
  active.logs = adapter.saveLogs(hl, path.join(jobsRoot, "job"));
  adapter.snapshot(active, { logs: active.logs.map(({ file, backup }) => ({ file, backup })) });
  fs.writeFileSync(path.join(hl, "qconsole.log"), "render output\r\n");
  adapter.finish(active);
  assert.equal(fs.existsSync(path.join(hl, "qconsole.log")), false, "до рендера журнала не было");
  assert.ok(gr.readMarker(jobsRoot, goldsrc.SPEC), "маркер ждёт реестра");
  assert.equal(gr.readMarker(jobsRoot, goldsrc.SPEC).logs, undefined);
  fs.writeFileSync(path.join(hl, "qconsole.log"), "player played after failed render\r\n");
  adapter.recover();
  assert.equal(fs.readFileSync(path.join(hl, "qconsole.log"), "utf8"), "player played after failed render\r\n");
  assert.deepEqual(keys.get(goldsrc.SPEC.registryKey), { ScreenWidth: 1920 });
});

test("mute после рендера HL1: когда сбрасывать — флаг, без рендера, без игры и чужого hl.exe, со Steam; сброс беззвучным запуском", { timeout: 60000 }, async (t) => {
  const s = setupAdapter(t);
  const { adapter, store, game } = s;
  assert.equal(adapter.muteResetBlocker(), "no-flag");
  store.set(goldsrc.SPEC.unmutePending, true);
  assert.equal(adapter.muteResetBlocker(), null);
  const pause = (field, value, code) => {
    const saved = adapter[field];
    adapter[field] = value;
    assert.equal(adapter.muteResetBlocker(), code, field);
    adapter[field] = saved;
  };
  pause("active", {}, "RENDER_ACTIVE");
  pause("gameRunning", () => true, "GAME_RUNNING");
  // Любой hl.exe (Steam HL1, CS 1.6, игра игрока вне лаунчера); список процессов не прочитан — тоже ждём.
  pause("inspector", { findByImageName: () => [{ pid: 5, executablePath: "D:\\cs\\hl.exe" }], samePath: () => false }, "GAME_RUNNING");
  pause("inspector", { findByImageName: () => { throw new Error("cim"); }, samePath: () => false }, "GAME_RUNNING");
  pause("steamStatus", () => ({ ok: false, code: "STEAM_NOT_RUNNING" }), "STEAM_NOT_RUNNING");
  pause("installPathFn", () => null, "GAME_NOT_INSTALLED");
  assert.equal(adapter.muteResetBlocker(), null);

  assert.equal(await adapter.resetMute(), true);
  assert.equal(game.launches.length, 1);
  const request = game.launches[0];
  assert.equal(request.resetOnly, true);
  assert.equal(request.resetExe, path.join(s.hl, "hl.exe"));
  assert.equal(request.cwd, s.hl);
  assert.deepEqual(argv(request.resetCommandLine).slice(1), gsr.resetArguments(), "hl.exe без Injector, bxt-rs и демок");
  assert.notEqual(store.get(goldsrc.SPEC.unmutePending), true);
  assertRestored(s);
  // Флага нет — ничего не запускается; рендер дожидается идущего сброса, а не отказывает.
  assert.equal(await adapter.resetMute(), false);
  assert.equal(game.launches.length, 1);
  store.set(goldsrc.SPEC.unmutePending, true);
  const resetting = adapter.resetMute();
  const material = await adapter.render(s.request);
  assert.equal(await resetting, true);
  assert.equal(material.video, path.join(s.jobDir, "material.mkv"));
});

test("сброс mute в лаунчере: при старте и раз в минуту — только не занятые игры, по одной; перед «Играть» — эта игра", async (t) => {
  const calls = [];
  const fake = (gameId, blocker, ok) => ({ id: gameId, blocker, ok, active: false, recover() {}, disposeSync() {}, cleanupStale: () => 0,
    isActive() { return this.active; }, muteResetBlocker() { return this.blocker; },
    async resetMute() { calls.push(gameId); return this.ok; } });
  const ghostingFake = fake("ghosting-renewed", null, false);
  const hl1Fake = fake("hl1-won", "STEAM_NOT_RUNNING", true);
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: path.join(tempDir(t), "jobs"), ghostingAdapter: ghostingFake, hl1Adapter: hl1Fake });
  t.after(() => tool.dispose());
  tool.startMuteResets();
  await tool.muteResetting;
  assert.deepEqual(calls, ["ghosting-renewed"], "HL1 ждёт Steam");
  // Сброс Ghosting не удался: в фоне больше не пробуем (игра не стартует каждую минуту), HL1 — как только можно.
  hl1Fake.blocker = null;
  await tool.resetPendingMutes();
  assert.deepEqual(calls, ["ghosting-renewed", "hl1-won"]);
  // Идёт рендер (любой игры) — фоновый сброс ждёт.
  hl1Fake.active = true;
  await tool.resetPendingMutes();
  assert.equal(calls.length, 2);
  hl1Fake.active = false;
  // Перед «Играть»: именно эта игра, даже после неудачного фонового сброса; занята — без сброса.
  assert.equal(await tool.resetMuteBeforeLaunch("ghosting-renewed"), false);
  assert.deepEqual(calls.slice(2), ["ghosting-renewed"]);
  assert.equal(await tool.resetMuteBeforeLaunch("hl2-4260"), false);
  ghostingFake.blocker = "GAME_RUNNING";
  assert.equal(await tool.resetMuteBeforeLaunch("ghosting-renewed"), false);
  assert.equal(calls.length, 3);
  assert.ok(tool.muteTimer, "таймер раз в минуту");
  tool.dispose();
  assert.equal(tool.muteTimer, null);

  // Запуск игры из лаунчера ждёт сброса; сбой сброса запуск не ломает.
  const electronPath = require.resolve("electron");
  require.cache[electronPath] = { id: electronPath, filename: electronPath, loaded: true, exports: { ipcMain: { handle() {}, on() {} } } };
  const GameDelivery = require("../src/main/game-delivery");
  const delivery = new GameDelivery({});
  const order = [];
  delivery.service = { isGameRunning: () => false };
  delivery.telemetryGameStarting = () => {};
  delivery.ensurePracticeSaves = () => {};
  delivery.launchService = { launch: async (gameId) => { order.push(`launch ${gameId}`); return { status: "started" }; } };
  delivery.beforeLaunch = async (gameId) => {
    await new Promise((resolve) => setTimeout(resolve, 20));
    order.push(`reset ${gameId}`);
  };
  assert.equal((await delivery.launch("hl1-won")).status, "started");
  assert.deepEqual(order, ["reset hl1-won", "launch hl1-won"]);
  delivery.beforeLaunch = async () => { throw new Error("helper"); };
  assert.equal((await delivery.launch("hl1-won")).status, "started");
  // Другие тесты подставляют свой ipcMain: модуль загрузится заново.
  delete require.cache[require.resolve("../src/main/game-delivery")];
});

test("RENDER_ACTIVE: рендер HL1 закрывает запуск и операции именно HL1", () => {
  const tool = new DemoTool({ store: fx.memoryStore(), ghostingAdapter: { isActive: () => false, recover() {}, disposeSync() {} },
    hl1Adapter: { isActive: () => true, recover() {}, disposeSync() {} } });
  assert.equal(tool.renderActive("hl1-won"), true);
  assert.equal(tool.renderActive("ghosting-renewed"), false);
  assert.equal(tool.renderActive("hl2-4260"), false);
});

// ---------- bxt-rs как скачиваемый инструмент ----------

test("манифест bxt-rs: закреплённые версия, размер и sha256, исходник рядом; на панели инструментов его нет", () => {
  const manifest = toolRegistry.DEPENDENCIES["bxt-rs"];
  assert.equal(manifest.raw, true);
  assert.equal(manifest.executable, "bxt_rs.dll");
  assert.equal(manifest.archive.size, 7006208);
  assert.equal(manifest.archive.sha256, "a598e9a626a69cfea65e9b539d6122ff5b8960bf3668011796ba1fd7de1dc505");
  assert.equal(manifest.files["bxt_rs.dll"], manifest.archive.sha256);
  assert.equal(manifest.license, "GPL-3.0-or-later");
  assert.deepEqual(manifest.archive.providers.map((provider) => provider.url), [`https://hlsr.pro/downloads/tools/bxt-rs/${manifest.version}/bxt_rs.dll`]);
  assert.equal(links.urlFor("bxtrsSource"), `https://hlsr.pro/downloads/tools/bxt-rs/${manifest.version}/bxt-rs-${manifest.version}-src.tar.gz`);
  // Лицензия GPL-3 и уведомления крейтов скачиваются рядом с DLL (I-3).
  const prefix = `https://hlsr.pro/downloads/tools/bxt-rs/${manifest.version}/`;
  assert.deepEqual(manifest.extras.map((extra) => [extra.name, extra.url, extra.size, extra.sha256]), [
    ["COPYING", `${prefix}COPYING`, 35148, "0ae0485a5bd37a63e63603596417e4eb0e653334fa6c7f932ca3a0e85d4af227"],
    ["THIRD-PARTY-NOTICES.txt", `${prefix}THIRD-PARTY-NOTICES.txt`, 202784, "a95fa3ed1e761f0a754f0dbc5496cbc70e1120795bb84e91faa179de863bdc01"],
  ]);
  assert.equal(links.urlFor("bxtrsLicense"), `${prefix}THIRD-PARTY-NOTICES.txt`);
  assert.equal(toolRegistry.manifestOf("bxt-rs"), null);
  assert.deepEqual(toolRegistry.ids(), ["livesplit"]);
  const notices = fs.readFileSync(path.join(__dirname, "../build/THIRD-PARTY-NOTICES.txt"), "utf8");
  for (const url of [links.urlFor("bxtrsSource"), `${prefix}COPYING`, `${prefix}THIRD-PARTY-NOTICES.txt`])
    assert.ok(notices.includes(url), `THIRD-PARTY-NOTICES лаунчера: ${url}`);
});

// Отдаёт файлы по имени из пути запроса.
function serve(t, files) {
  return new Promise((resolve) => {
    const server = http.createServer((request, response) => {
      const body = files[decodeURIComponent(request.url.slice(1))];
      if (!body) {
        response.writeHead(404);
        return response.end();
      }
      response.writeHead(200, { "Content-Length": body.length });
      response.end(body);
    });
    server.listen(0, "127.0.0.1", () => {
      t.after(() => server.close());
      resolve(`http://127.0.0.1:${server.address().port}/`);
    });
  });
}

test("установка bxt-rs: сама DLL без распаковки и её лицензии рядом, sha256 сверен; подмена — CHECKSUM_MISMATCH; статус и путь для рендера", async (t) => {
  const root = tempDir(t);
  const body = Buffer.from("MZ fake bxt_rs.dll for the test");
  const sha = (data) => crypto.createHash("sha256").update(data).digest("hex");
  const sha256 = sha(body);
  const texts = { COPYING: Buffer.from("GNU GENERAL PUBLIC LICENSE\n"), "THIRD-PARTY-NOTICES.txt": Buffer.from("crate notices\n") };
  const base = await serve(t, Object.assign({ "bxt_rs.dll": body }, texts));
  const local = JSON.parse(JSON.stringify(toolRegistry.DEPENDENCIES["bxt-rs"]));
  Object.assign(local.archive, { size: body.length, sha256, providers: [{ id: "local", type: "https-direct", url: `${base}bxt_rs.dll` }] });
  local.files = { "bxt_rs.dll": sha256 };
  local.extras = Object.entries(texts).map(([name, data]) => ({ name, url: base + encodeURIComponent(name), size: data.length, sha256: sha(data) }));
  const paths = { root, tools: path.join(root, "Tools"), staging: path.join(root, ".staging"), downloads: path.join(root, ".downloads") };
  const tools = new ToolService({ paths, registry: { "bxt-rs": local } });
  const percents = [];
  const tool = new DemoTool({ store: fx.memoryStore(), tools: () => tools, platform: "win32", production: true, jobsRoot: path.join(root, "jobs"),
    emit: (channel, payload) => percents.push([channel, payload.percent]) });
  const status = tool.bxtrsStatus();
  assert.deepEqual([status.state, status.sourceLink, status.licenseLink, tool.bxtrsLocated()], ["not-installed", "bxtrsSource", "bxtrsLicense", null]);
  // Размер в карточке — DLL и тексты лицензий.
  const manifest = toolRegistry.DEPENDENCIES["bxt-rs"];
  assert.equal(status.sizeBytes, manifest.archive.size + manifest.extras.reduce((sum, extra) => sum + extra.size, 0));
  const installed = await tool.installBxtrs();
  assert.equal(installed.bxtrs.state, "ready");
  assert.equal(installed.bxtrs.source, "tool");
  assert.deepEqual(percents[percents.length - 1], ["demo-bxtrs-progress", 100]);
  const dir = path.join(paths.tools, "bxt-rs");
  assert.deepEqual(fs.readdirSync(dir).sort(), ["COPYING", "THIRD-PARTY-NOTICES.txt", "bxt_rs.dll", "installed.json"]);
  assert.equal(fs.readFileSync(path.join(dir, "COPYING"), "utf8"), "GNU GENERAL PUBLIC LICENSE\n");
  assert.deepEqual(fs.readdirSync(paths.downloads), []);
  // Рендеру — путь и закреплённый sha256 из манифеста лаунчера (адаптер сверит перед внедрением).
  assert.deepEqual(tool.bxtrsLocated(), { path: path.join(dir, "bxt_rs.dll"), sha256: toolRegistry.DEPENDENCIES["bxt-rs"].files["bxt_rs.dll"], source: "tool" });

  const badRoot = tempDir(t);
  const bad = new ToolService({ paths: { root: badRoot, tools: path.join(badRoot, "Tools"), staging: path.join(badRoot, ".staging"), downloads: path.join(badRoot, ".downloads") },
    registry: { "bxt-rs": Object.assign({}, local, { files: { "bxt_rs.dll": "0".repeat(64) } }) } });
  await assert.rejects(bad.install("bxt-rs"), { code: "CHECKSUM_MISMATCH" });
  assert.equal(fs.existsSync(path.join(badRoot, "Tools", "bxt-rs")), false);
  // Подменённый текст лицензии или файл без размера — инструмент не ставится.
  for (const [extras, code] of [[[Object.assign({}, local.extras[0], { sha256: "0".repeat(64) })], "CHECKSUM_MISMATCH"],
    [[Object.assign({}, local.extras[0], { size: undefined })], "MANIFEST_INVALID"]]) {
    const other = tempDir(t);
    const service = new ToolService({ paths: { root: other, tools: path.join(other, "Tools"), staging: path.join(other, ".staging"), downloads: path.join(other, ".downloads") },
      registry: { "bxt-rs": Object.assign({}, local, { extras }) } });
    await assert.rejects(service.install("bxt-rs"), { code }, code);
    assert.equal(fs.existsSync(path.join(other, "Tools", "bxt-rs")), false);
  }
});

test("bxt-rs: подмена HLSR_DEMO_BXTRS только вне production; Linux — не ставится; общие tool-* его не принимают", async (t) => {
  const root = tempDir(t);
  const dll = path.join(root, "bxt_rs.dll");
  fs.writeFileSync(dll, "x");
  const dev = new DemoTool({ store: fx.memoryStore(), platform: "win32", production: false, env: { HLSR_DEMO_BXTRS: root }, jobsRoot: path.join(root, "jobs") });
  assert.deepEqual(dev.bxtrsLocated(), { path: dll, sha256: null, source: "env" });
  const prod = new DemoTool({ store: fx.memoryStore(), platform: "win32", production: true, env: { HLSR_DEMO_BXTRS: root }, jobsRoot: path.join(root, "jobs") });
  assert.equal(prod.bxtrsLocated(), null);
  // Установлена другая версия — как не установлена (лаунчер ждёт свою).
  const old = new DemoTool({ store: fx.memoryStore(), platform: "win32", production: true, jobsRoot: path.join(root, "jobs"),
    tools: () => ({ readInstalled: () => ({ version: "won-old" }), installPathOf: () => root }) });
  assert.equal(old.bxtrsLocated(), null);
  const linux = new DemoTool({ store: fx.memoryStore(), platform: "linux", production: true, jobsRoot: path.join(root, "jobs") });
  await assert.rejects(linux.installBxtrs(), { code: "BXTRS_INSTALL_UNSUPPORTED" });
  assert.equal(linux.bxtrsStatus().installable, false);

  const electronPath = require.resolve("electron");
  const handlers = new Map();
  require.cache[electronPath] = { id: electronPath, filename: electronPath, loaded: true,
    exports: { ipcMain: { handle: (channel, fn) => handlers.set(channel, fn), on() {} } } };
  const GameDelivery = require("../src/main/game-delivery");
  const delivery = new GameDelivery({});
  let touched = 0;
  delivery.tools = new Proxy({}, { get: () => () => { touched++; return { state: "ready" }; } });
  delivery.initialize();
  for (const channel of ["tool-launch", "tool-stop", "tool-remove", "tool-open-folder", "tool-install"])
    assert.deepEqual(await handlers.get(channel)({}, "bxt-rs"), { status: "error", error: { code: "TOOL_UNKNOWN" } }, channel);
  assert.equal(touched, 0);
});

test("метки до рендера HL1 — те же, что даст рендер: смена карты внутри демки из самой демки", async (t) => {
  const dir = tempDir(t);
  const [one, two] = runDemos(dir);
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: path.join(dir, "jobs"), production: true });
  const added = tool.addPaths([one, two]);
  const inspected = await tool.inspect(added.added[0]);
  assert.deepEqual(inspected.markers, [{ t: 0, type: "map", label: "c1a0" }, { t: 2, type: "saveload", label: "2" }, { t: 3, type: "map", label: "c1a0d" }]);
});

// ---------- помощник по-настоящему (скрытый стол, без игры) ----------

// PID процессов с этим образом (только чтение, tasklist).
function pidsOf(image) {
  const out = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${image}`, "/FO", "CSV", "/NH"], { encoding: "utf8", windowsHide: true }).stdout || "";
  return new Set([...out.matchAll(/^"[^"]+","(\d+)"/gm)].map((match) => Number(match[1])));
}

test("помощник: followJob — посредник вышел, а «игра» в задании жива, exited только после неё; watchImage — foreign только для процесса вне задания", { timeout: 120000 }, async (t) => {
  if (process.platform !== "win32") return t.skip("ПРОПУЩЕНО: помощник только для Windows");
  const { RenderHost, scriptPath } = require("../src/main/demo-tool/render-host");
  const system = path.join(process.env.SystemRoot || "C:\\Windows", "System32");
  const cmd = path.join(system, "cmd.exe");
  const ping = path.join(system, "PING.EXE");
  const before = pidsOf("PING.EXE");
  // «Чужая игра»: свой ping вне задания помощника (закрывается по своему дескриптору).
  const outside = require("node:child_process").spawn(ping, ["-n", "30", "127.0.0.1"], { windowsHide: true, stdio: "ignore" });
  t.after(() => outside.kill());
  const host = new RenderHost({ script: scriptPath(null), env: process.env });
  const events = [];
  host.on("event", (message) => events.push(Object.assign({ at: Date.now() }, message)));
  t.after(() => host.kill());
  // Посредник (cmd) запускает «игру» (ping ~3 с) и сразу выходит, как Injector.
  const pid = await host.start({ exe: cmd, cwd: os.tmpdir(), commandLine: `"${cmd}" /c start "" /b "${ping}" -n 4 127.0.0.1 >nul`,
    mute: false, followJob: true, watchImage: "PING" });
  assert.ok(pid > 0);
  const startedAt = Date.now();
  for (const until = Date.now() + 30000; host.exitCode === null && Date.now() < until;) await new Promise((resolve) => setTimeout(resolve, 50));
  assert.notEqual(host.exitCode, null, "exited пришёл");
  const exitedAfter = events.find((message) => message.event === "exited").at - startedAt;
  assert.ok(exitedAfter > 1500, `exited через ${exitedAfter} мс — раньше конца «игры» в задании`);
  await host.stop(false);
  const foreign = events.filter((message) => message.event === "foreign").map((message) => message.pid);
  assert.ok(foreign.includes(outside.pid), `foreign: ${foreign}`);
  // ping из задания (его PID новый, не outside) — не foreign.
  assert.deepEqual(foreign.filter((id) => id !== outside.pid && !before.has(id)), []);
});

// ---------- настоящая игра ----------

test("настоящая игра: короткий рендер HL1 на скрытом столе (HLSR_DEMO_HL1_GAME, HLSR_DEMO_HL1_DEMOS, HLSR_DEMO_BXTRS)", { timeout: 300000 }, async (t) => {
  const game = process.env.HLSR_DEMO_HL1_GAME;
  const demos = String(process.env.HLSR_DEMO_HL1_DEMOS || "").split(";").filter(Boolean);
  const dll = process.env.HLSR_DEMO_BXTRS;
  if (!game || !demos.length || !dll || !ff || process.platform !== "win32")
    return t.skip("ПРОПУЩЕНО: нужны копия hl1-won (HLSR_DEMO_HL1_GAME), демки через «;» (HLSR_DEMO_HL1_DEMOS), bxt_rs.dll (HLSR_DEMO_BXTRS), FFmpeg (HLSR_DEMO_FFMPEG), Windows и запущенный Steam");
  const SteamLocator = require("../src/main/game-delivery/SteamLocator");
  const { localConfigOf } = require("../src/main/game-delivery/manifests");
  const dir = tempDir(t);
  const steam = new SteamLocator({ logger: { info() {}, warn() {}, error() {} } });
  const done = {};
  const finished = new Promise((resolve) => { done.resolve = resolve; });
  const tool = new DemoTool({
    store: fx.memoryStore(),
    games: () => [{ gameId: "hl1-won", installPath: game, config: localConfigOf("hl1-won") }],
    jobsRoot: path.join(dir, "jobs"),
    videosDir: dir,
    production: false,
    env: Object.assign({}, process.env, { HLSR_DEMO_FFMPEG: ff.dir, HLSR_DEMO_BXTRS: dll }),
    steamStatus: () => steam.describeSteamOnly(),
    screenSize: () => ({ width: 1280, height: 720 }),
    emit: (channel, payload) => {
      if (channel === "demo-job-done" || channel === "demo-job-error") done.resolve(payload);
    },
  });
  t.after(() => tool.dispose());
  tool.storeSet("outputDir", dir);
  const id = tool.addPaths(demos).added[0];
  const started = await tool.startExport(id, { preset: "quality", height: 720, fps: 30, audio: true, keyhud: true, encoder: "cpu" });
  assert.equal(started.status, "ok");
  const payload = await finished;
  assert.equal(payload.error, undefined, JSON.stringify(payload.error));
  const probe = fx.ffprobeJson(ff.ffprobe, payload.result.file);
  assert.deepEqual(probe.streams.map((stream) => stream.codec_name).sort(), ["aac", "h264"]);
  assert.equal(fs.existsSync(path.join(game, "Half-Life", "valve_WON", "hlsr_render")), false);
  assert.equal(fs.existsSync(path.join(game, "Bunnymod XT", "bxt_rs.dll")), false);
});
