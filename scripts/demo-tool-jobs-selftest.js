"use strict";

// «Демо»: задания с подставным адаптером и настоящим FFmpeg (HLSR_DEMO_FFMPEG
// — папка bin сборки 9.0.2; без него тесты с FFmpeg пропускаются), протокол
// hlsr-media, чистка папок заданий.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const DemoTool = require("../src/main/demo-tool");
const adapters = require("../src/main/demo-tool/adapters");
const ffmpeg = require("../src/main/demo-tool/ffmpeg");
const jobs = require("../src/main/demo-tool/jobs");
const media = require("../src/main/demo-tool/media");
const planner = require("../src/main/demo-tool/planner");
const { localConfigOf } = require("../src/main/game-delivery/manifests");
const fx = require("./lib/demo-fixtures");

const ff = fx.findFfmpeg();
const NO_FFMPEG = "ПРОПУЩЕНО: нет FFmpeg — задайте HLSR_DEMO_FFMPEG (папка bin FFmpeg 9.0.2), иначе задания с FFmpeg не проверены";

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-demo-jobs-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));
  return dir;
}

function setup(t, options) {
  const opts = options || {};
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  fx.writeGhostingRun(path.join(install, "ghostingmod", "demos"), "myrun1",
    opts.run || [{ map: "d1_trainstation_01", duration: 6 }, { map: "d1_trainstation_02", duration: 6 }]);
  const events = [];
  const waiters = [];
  const spawned = [];
  const tool = new DemoTool({
    store: fx.memoryStore(),
    games: () => [{ gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") }],
    jobsRoot: path.join(dir, "jobs"),
    videosDir: path.join(dir, "Videos"),
    env: ff && ff.dir ? { [ffmpeg.ENV_OVERRIDE]: ff.dir } : {},
    production: false,
    adapters: opts.fake === false ? undefined : [adapters.fakeAdapter({ ffmpeg: ff && ff.ffmpeg, onSpawn: (child) => spawned.push(child) })],
    ghostingAdapter: opts.ghostingAdapter,
    limitBytes: opts.limitBytes,
    shell: opts.shell,
    emit: (channel, payload) => {
      events.push({ channel, payload });
      for (const waiter of waiters.slice()) if (waiter.match(channel, payload)) {
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(payload);
      }
    },
  });
  const waitFor = (match) => new Promise((resolve) => waiters.push({ match, resolve }));
  return { dir, tool, events, spawned, waitFor, id: tool.list().items[0].id, jobsRoot: path.join(dir, "jobs") };
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return false;
  }
};

test("экспорт: интервал и скорость, MP4 проходит ffprobe, timeline.json пересчитан, папка задания удалена", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, events, id, jobsRoot, dir } = setup(t);
  const started = await tool.startExport(id, { preset: "quality", height: 720, fps: 30, encoder: "cpu", speed: 2, range: { from: 2, to: 10 } });
  assert.equal(started.status, "ok");
  assert.equal(started.plan.encoder, "libx264");
  const done = await tool.lastJob;
  assert.ok(done.result, JSON.stringify(done));
  const { result } = done;
  assert.equal(result.encoder, "libx264");
  assert.equal(result.fileName, "myrun1_00-02_00-10_x2.mp4");
  assert.equal(path.dirname(result.file), path.join(dir, "Videos", "HLSR"));
  assert.equal(result.timelineFile, "myrun1_00-02_00-10_x2.timeline.json");

  const info = fx.ffprobeJson(ff.ffprobe, result.file);
  const video = info.streams.find((stream) => stream.codec_type === "video");
  const audio = info.streams.find((stream) => stream.codec_type === "audio");
  assert.equal(video.codec_name, "h264");
  assert.equal(video.avg_frame_rate, "30/1");
  assert.equal(video.width, 1280);
  assert.equal(video.height, 720);
  assert.equal(audio.codec_name, "aac");
  assert.ok(Math.abs(Number(info.format.duration) - 4) < 0.15, info.format.duration);

  const timeline = JSON.parse(fs.readFileSync(path.join(path.dirname(result.file), result.timelineFile), "utf8"));
  assert.equal(timeline.version, 1);
  // id — запись списка «Демо»: по нему «Отрендеренные» находят исходную демку.
  assert.deepEqual(timeline.source, { id, files: ["run.ghplaylist", "segment001.dem", "segment002.dem"], engine: "source", game: "ghosting", map: "d1_trainstation_01" });
  assert.equal(timeline.video.file, result.fileName);
  assert.deepEqual([timeline.video.fps, timeline.video.width, timeline.video.height, timeline.video.speed, timeline.video.encoder], [30, 1280, 720, 2, "libx264"]);
  assert.deepEqual(timeline.video.range, { from: 2, to: 10 });
  assert.deepEqual(timeline.markers, [
    { t: 0, type: "map", label: "d1_trainstation_01", sourceTime: 2 },
    { t: 2, type: "map", label: "d1_trainstation_02", sourceTime: 6 },
    { t: 2, type: "timer", label: "0:06.010", sourceTime: 6 },
    { t: 2, type: "timer", label: "fake", sourceTime: 6 },
  ]);
  assert.deepEqual(tool.readTimeline(result.id).timeline, timeline);

  const phases = events.filter((event) => event.channel === "demo-job-progress").map((event) => event.payload);
  assert.deepEqual([...new Set(phases.map((item) => item.phase))], ["prepare", "render", "encode", "verify", "done"]);
  for (let i = 1; i < phases.length; i++) assert.ok(phases[i].percent >= phases[i - 1].percent);
  assert.equal(events[events.length - 1].channel, "demo-job-done");
  assert.deepEqual(fs.readdirSync(jobsRoot), []);
  assert.deepEqual(tool.savedSettings(), { preset: "quality", height: 720, fps: 30, audio: true, keyhud: false, integrity: false, encoder: "cpu", speed: 2,
    hud: true, ownConfig: true, ownHud: true });

  // Тот же экспорт ещё раз — имя с суффиксом, без перезаписи.
  await tool.startExport(id, { preset: "quality", height: 720, fps: 30, encoder: "cpu", speed: 2, range: { from: 2, to: 10 } });
  assert.equal((await tool.lastJob).result.fileName, "myrun1_00-02_00-10_x2 (2).mp4");

  // Плеер: файл итога отдаётся по токену, перемотка — 206 на Range.
  const url = tool.mediaUrl(result.id).url;
  const size = fs.statSync(result.file).size;
  const ranged = await tool.media.handle(new Request(url, { headers: { Range: "bytes=100-1123" } }));
  assert.equal(ranged.status, 206);
  assert.equal(ranged.headers.get("content-range"), `bytes 100-1123/${size}`);
  assert.equal(ranged.headers.get("content-type"), "video/mp4");
  const body = Buffer.from(await ranged.arrayBuffer());
  assert.deepEqual(body, fs.readFileSync(result.file).subarray(100, 1124));
});

test("Discord: длинный клип укладывается в 10 000 000 байт", { timeout: 240000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id } = setup(t, { run: [{ map: "d1_canals_01", duration: 60 }, { map: "d1_canals_01a", duration: 60 }] });
  const started = await tool.startExport(id, { preset: "discord", height: 720, fps: 30, encoder: "cpu" });
  assert.equal(started.plan.twoPass, true);
  const { result, error } = await tool.lastJob;
  assert.ok(result, JSON.stringify(error));
  assert.ok(result.size <= 10000000, String(result.size));
  const info = fx.ffprobeJson(ff.ffprobe, result.file);
  assert.ok(Math.abs(Number(info.format.duration) - 120) < 0.5);
  assert.equal(info.streams.find((stream) => stream.codec_type === "audio").codec_name, "aac");
});

test("Discord: превышение лимита — перекодирование с меньшим битрейтом, после двух повторов — совет укоротить", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id, dir } = setup(t);
  const item = tool.resolve(id);
  const settings = planner.normalizeSettings({ preset: "discord", height: 720, fps: 30, encoder: "cpu", range: { from: 0, to: 4 } }, item.duration);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 4 });
  const root = path.join(dir, "jobs");
  const outputDir = path.join(dir, "out");
  // Кодировщик «промахнулся»: к итогу дописываются лишние байты.
  const overshooting = (times) => {
    const bitrates = [];
    let left = times;
    const runFn = async (exe, args, options) => {
      const done = await ffmpeg.run(exe, args, options);
      const output = args[args.length - 1];
      if (output.endsWith("output.mp4")) {
        bitrates.push(Number(args[args.indexOf("-b:v") + 1]));
        if (left-- > 0) fs.appendFileSync(output, Buffer.alloc(plan.limitBytes));
      }
      return done;
    };
    return { runFn, bitrates };
  };
  const run = (id2, runFn) => jobs.runJob({
    id: id2, kind: "export", item, adapter: adapters.fakeAdapter({ ffmpeg: ff.ffmpeg }), settings, plan, root, outputDir,
    ffmpegPath: ff.ffmpeg, ffprobePath: ff.ffprobe, signal: new AbortController().signal, runFn,
  });

  const once = overshooting(1);
  const result = await run("retry-once", once.runFn);
  assert.equal(result.attempts, 2);
  assert.ok(result.size <= plan.limitBytes);
  assert.equal(once.bitrates.length, 2);
  assert.ok(once.bitrates[1] < once.bitrates[0]);

  const always = overshooting(Infinity);
  await assert.rejects(run("retry-always", always.runFn), (error) =>
    error.code === "DISCORD_TOO_LONG" && error.details.reason === "retries" && error.details.recommendedMaxSeconds > 0);
  assert.equal(always.bitrates.length, 3, "первая попытка и два повтора");
  assert.deepEqual(fs.readdirSync(root), []);
  assert.deepEqual(fs.readdirSync(outputDir).filter((name) => name.startsWith("myrun1") && name.includes("(2)")), []);
});

test("отмена посреди рендера: процесс адаптера погашен, папка задания удалена", { timeout: 60000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id, spawned, waitFor, jobsRoot, dir } = setup(t, { run: [{ map: "d1_canals_01", duration: 600 }] });
  const rendering = waitFor((channel, payload) => channel === "demo-job-progress" && payload.phase === "render" && payload.percent > 2);
  const started = await tool.startExport(id, { preset: "quality", height: 1080, fps: 60, encoder: "cpu" });
  await rendering;
  assert.equal(fs.readdirSync(jobsRoot).length, 1);
  assert.deepEqual(tool.cancel("other-job"), { status: "ok", cancelled: false });
  assert.deepEqual(tool.cancel(started.jobId), { status: "ok", cancelled: true });
  const outcome = await tool.lastJob;
  assert.deepEqual(outcome.error, { code: "CANCELLED" });
  assert.equal(spawned.length, 1);
  assert.equal(alive(spawned[0].pid), false);
  assert.deepEqual(fs.readdirSync(jobsRoot), []);
  assert.equal(fs.existsSync(path.join(dir, "Videos", "HLSR")) ? fs.readdirSync(path.join(dir, "Videos", "HLSR")).length : 0, 0);
  assert.equal(tool.job, null);
});

test("отмена посреди кодирования: FFmpeg погашен по PID, папка задания удалена", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id, dir } = setup(t, { run: [{ map: "d1_canals_01", duration: 40 }] });
  const item = tool.resolve(id);
  const settings = planner.normalizeSettings({ preset: "quality", height: 1080, fps: 60, encoder: "cpu" }, item.duration);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: item.duration });
  const controller = new AbortController();
  const root = path.join(dir, "jobs");
  const encoders = [];
  const pending = jobs.runJob({
    id: "cancel-encode", kind: "export", item, adapter: adapters.fakeAdapter({ ffmpeg: ff.ffmpeg }), settings, plan, root,
    outputDir: path.join(dir, "out"), ffmpegPath: ff.ffmpeg, ffprobePath: ff.ffprobe, signal: controller.signal,
    onSpawn: (child) => encoders.push(child),
    onProgress: (progress) => {
      if (progress.phase === "encode" && progress.percent > 50) controller.abort();
    },
  });
  await assert.rejects(pending, { code: "CANCELLED" });
  assert.equal(encoders.length, 1);
  assert.equal(alive(encoders[0].pid), false);
  assert.equal(fs.existsSync(path.join(root, "cancel-encode")), false);
  assert.equal(fs.existsSync(path.join(dir, "out")), false);
});

test("предпросмотр: 720p30 во временной папке, хранится один, удаляется по запросу", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id, jobsRoot } = setup(t);
  await tool.startPreview(id, { range: { from: 1, to: 4 }, height: 2160, preset: "discord" });
  const first = await tool.lastJob;
  assert.equal(first.kind, "preview");
  assert.equal(first.result.file, null);
  assert.deepEqual([first.result.video.width, first.result.video.height, first.result.video.fps], [1280, 720, 30]);
  const firstDir = path.join(jobsRoot, first.jobId);
  // Пока предпросмотр открыт, в папке только итог: сырьё адаптера удалено сразу после кодирования.
  assert.deepEqual(fs.readdirSync(firstDir), ["output.mp4"]);
  const info = fx.ffprobeJson(ff.ffprobe, path.join(firstDir, "output.mp4"));
  assert.equal(info.streams.find((stream) => stream.codec_type === "video").avg_frame_rate, "30/1");
  assert.ok(info.streams.some((stream) => stream.codec_type === "audio"));
  const url = first.result.mediaUrl;

  await tool.startPreview(id, { range: { from: 0, to: 2 } });
  const second = await tool.lastJob;
  assert.equal(fs.existsSync(firstDir), false, "старый предпросмотр удалён");
  assert.equal((await tool.media.handle(new Request(url))).status, 404);
  assert.deepEqual(fs.readdirSync(jobsRoot), [second.jobId]);
  assert.throws(() => tool.reveal(second.jobId), { code: "RESULT_NOT_FOUND" });

  assert.deepEqual(tool.clearPreview(), { status: "ok", cleared: true });
  assert.deepEqual(fs.readdirSync(jobsRoot), []);
  assert.equal((await tool.media.handle(new Request(second.result.mediaUrl))).status, 404);
});

test("игру рендерить нельзя (не установлена): план считается, экспорт — отказ с кодом; пробы кодировщиков и сочетания", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  // Адаптер Ghosting без установленной игры; монитор 4K — все 12 сочетаний.
  const { GhostingAdapter } = require("../src/main/demo-tool/ghosting");
  const refusal = process.platform === "win32" ? "GAME_NOT_INSTALLED" : "RENDER_NOT_READY";
  const { tool, id } = setup(t, { fake: false, ghostingAdapter: new GhostingAdapter({ installPath: () => null, screenSize: () => ({ width: 3840, height: 2160 }) }) });
  const status = await tool.ffmpegStatus();
  assert.equal(status.state, "ready");
  assert.equal(status.source, "env");
  assert.equal(status.license, "GPL");
  const encoders = await tool.encoders();
  assert.equal(encoders.cpu, "libx264");
  assert.equal(encoders.auto, encoders.gpu || "libx264");
  const options = await tool.options(id);
  assert.deepEqual(options.render, { ok: false, code: refusal });
  assert.deepEqual(options.keyhud, { available: true, code: null });
  // «Дополнительные настройки» Ghosting: конфиг и HUD игрока — да, HUD целиком — нет (только в моде).
  assert.deepEqual([options.capabilities.keyhud, options.capabilities.hud, options.capabilities.ownConfig, options.capabilities.ownHud], [true, false, true, true]);
  const prepared = await tool.prepare(tool.resolve(id), { encoder: "cpu", height: 720, fps: 30, hud: false, ownConfig: false, ownHud: false });
  assert.deepEqual([prepared.settings.hud, prepared.settings.ownConfig, prepared.settings.ownHud], [true, false, false], "чего игра не умеет — как у игрока");
  assert.equal(options.modes.cpu.encoder, "libx264");
  assert.equal(options.modes.cpu.combos.length, 12);
  if (encoders.gpu) assert.ok(options.modes.gpu.combos.length > 0);
  else assert.equal(options.modes.gpu, null);
  const plan = await tool.plan(id, { preset: "discord", height: 720, fps: 30, encoder: "cpu" });
  assert.equal(plan.status, "ok");
  assert.equal(plan.render.code, refusal);
  await assert.rejects(tool.startExport(id, { encoder: "cpu" }), { code: refusal });
  assert.equal(tool.job, null);
});

test("GPU: экспорт Discord фактическим GPU-кодировщиком", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id } = setup(t);
  const encoders = await tool.encoders();
  if (!encoders.gpu) return t.skip("GPU-кодировщика нет");
  await tool.startExport(id, { preset: "discord", height: 1080, fps: 60, encoder: "gpu" });
  const { result, error } = await tool.lastJob;
  assert.ok(result, JSON.stringify(error));
  assert.equal(result.encoder, encoders.gpu);
  assert.ok(result.size <= 10000000);
  const info = fx.ffprobeJson(ff.ffprobe, result.file);
  assert.equal(info.streams.find((stream) => stream.codec_type === "video").codec_name, "h264");
  assert.equal(info.streams.find((stream) => stream.codec_type === "video").avg_frame_rate, "60/1");
});

test("hlsr-media: только зарегистрированные файлы, Range, 404/405/416", async (t) => {
  const dir = tempDir(t);
  const file = path.join(dir, "clip.mp4");
  const data = Buffer.from(Array.from({ length: 5000 }, (_, i) => i % 251));
  fs.writeFileSync(file, data);
  const registry = new media.MediaRegistry();
  const { url, token } = registry.register(file);
  assert.match(url, /^hlsr-media:\/\/media\/[0-9a-f]{32}$/);

  const full = await registry.handle(new Request(url));
  assert.equal(full.status, 200);
  assert.equal(full.headers.get("accept-ranges"), "bytes");
  assert.equal(full.headers.get("content-length"), "5000");
  assert.deepEqual(Buffer.from(await full.arrayBuffer()), data);

  const tail = await registry.handle(new Request(url, { headers: { Range: "bytes=-100" } }));
  assert.equal(tail.status, 206);
  assert.equal(tail.headers.get("content-range"), "bytes 4900-4999/5000");
  assert.deepEqual(Buffer.from(await tail.arrayBuffer()), data.subarray(4900));
  const open = await registry.handle(new Request(url, { headers: { Range: "bytes=4990-" } }));
  assert.equal(open.headers.get("content-length"), "10");

  assert.equal((await registry.handle(new Request(url, { headers: { Range: "bytes=6000-" } }))).status, 416);
  assert.equal((await registry.handle(new Request(url, { headers: { Range: "items=0-1" } }))).status, 416);
  assert.equal((await registry.handle(new Request(url, { method: "POST", body: "x" }))).status, 405);
  assert.equal((await registry.handle(new Request(`hlsr-media://media/${"0".repeat(32)}`))).status, 404);
  assert.equal((await registry.handle(new Request(`hlsr-media://other/${token}`))).status, 404);
  assert.equal((await registry.handle(new Request("hlsr-media://media/..%2F..%2Fwindows%2Fwin.ini"))).status, 404);
  const head = await registry.handle(new Request(url, { method: "HEAD" }));
  assert.equal(head.status, 200);
  assert.equal(head.headers.get("content-length"), "5000");
  registry.unregister(token);
  assert.equal((await registry.handle(new Request(url))).status, 404);
  assert.equal(media.parseRange("bytes=0-0", 1).end, 0);
  assert.equal(media.parseRange(null, 10), null);
});

test("проверка итога: 2 с видео при 10 с звука — VERIFY_FAILED (apad не маскирует обрезанное видео)", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id, dir } = setup(t);
  const item = tool.resolve(id);
  const settings = planner.normalizeSettings({ preset: "quality", height: 720, fps: 30, encoder: "cpu", range: { from: 0, to: 10 } }, item.duration);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 10 });
  const quiet = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"];
  // Игра «упала» на второй секунде: кадров 2 с, звука 10 с.
  const shortVideo = {
    id: "short",
    engine: "any",
    capabilities: adapters.PENDING_CAPS,
    canRender: () => ({ ok: true }),
    async render({ jobDir, signal }) {
      const video = path.join(jobDir, "v.mkv");
      const audio = path.join(jobDir, "a.wav");
      await ffmpeg.run(ff.ffmpeg, quiet.concat(["-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=2", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", video]), { signal });
      await ffmpeg.run(ff.ffmpeg, quiet.concat(["-f", "lavfi", "-i", "sine=duration=10:sample_rate=48000", "-c:a", "pcm_s16le", audio]), { signal });
      return { video, audio, offset: 0, timeline: [], cuts: [] };
    },
  };
  const root = path.join(dir, "jobs");
  await assert.rejects(jobs.runJob({
    id: "short-video", kind: "export", item, adapter: shortVideo, settings, plan, root, outputDir: path.join(dir, "out"),
    ffmpegPath: ff.ffmpeg, ffprobePath: ff.ffprobe, signal: new AbortController().signal,
  }), (error) => error.code === "VERIFY_FAILED" && /video-duration 2(\.|$|,)/.test(error.message));
  assert.equal(fs.existsSync(path.join(dir, "out")), false, "итог не сохранён");
  assert.deepEqual(fs.readdirSync(root), []);
});

test("поздняя отмена старого задания не гасит новое, пока оно готовится; отмена без id — гасит", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const { tool, id } = setup(t);
  const settings = { preset: "quality", height: 720, fps: 30, encoder: "cpu", range: { from: 0, to: 2 } };
  const starting = tool.startExport(id, settings);
  assert.equal(tool.job.id, null, "задание ещё готовится");
  assert.deepEqual(tool.cancel("old-job-id"), { status: "ok", cancelled: false });
  assert.equal((await starting).status, "ok");
  assert.ok((await tool.lastJob).result, "новое задание доделано");
  const second = tool.startExport(id, settings);
  assert.deepEqual(tool.cancel(), { status: "ok", cancelled: true });
  await assert.rejects(second, { code: "CANCELLED" });
  assert.equal(tool.job, null);
});

test("вырезанное адаптером накрыло весь отрезок — RENDER_FAILED до кодирования", async (t) => {
  const dir = tempDir(t);
  const root = path.join(dir, "jobs");
  const adapter = {
    id: "all-cut",
    engine: "any",
    capabilities: adapters.PENDING_CAPS,
    canRender: () => ({ ok: true }),
    async render({ jobDir }) {
      const video = path.join(jobDir, "v.mkv");
      fs.writeFileSync(video, "x");
      return { video, audio: null, offset: 0, timeline: [], cuts: [{ from: 0, to: 20 }] };
    },
  };
  const settings = planner.normalizeSettings({ range: { from: 2, to: 8 } }, 10);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 6 });
  await assert.rejects(jobs.runJob({
    id: "all-cut", kind: "export", item: { name: "x", duration: 10, segments: [] }, adapter, settings, plan, root,
    outputDir: path.join(dir, "out"), ffmpegPath: "ffmpeg", signal: new AbortController().signal,
    runFn: () => assert.fail("FFmpeg не должен запускаться"),
  }), { code: "RENDER_FAILED" });
  assert.deepEqual(fs.readdirSync(root), []);
});

test("корень заданий: junction или файл вместо корня, чужой владелец (логика Linux) — отказ; без корня — JOBS_DIR_UNAVAILABLE", async (t) => {
  const dir = tempDir(t);
  const outside = path.join(dir, "outside");
  fs.mkdirSync(outside);
  fs.writeFileSync(path.join(outside, "keep.txt"), "x");
  const linkRoot = path.join(dir, "link-root");
  fs.symlinkSync(outside, linkRoot, "junction");
  assert.throws(() => jobs.checkRoot(linkRoot), { code: "JOBS_DIR_UNSAFE" });
  assert.throws(() => jobs.prepareRoot(linkRoot), { code: "JOBS_DIR_UNSAFE" });
  assert.throws(() => jobs.cleanupStale(linkRoot, Date.now() + 3 * 24 * 3600 * 1000), { code: "JOBS_DIR_UNSAFE" });
  assert.ok(fs.existsSync(path.join(outside, "keep.txt")), "по ссылке-корню ничего не удалено");
  const tool = new DemoTool({ store: fx.memoryStore(), jobsRoot: linkRoot, production: true });
  await assert.rejects(tool.startExport("x", {}), { code: "JOBS_DIR_UNSAFE" });
  assert.equal(tool.job, null);

  const fileRoot = path.join(dir, "file-root");
  fs.writeFileSync(fileRoot, "");
  assert.throws(() => jobs.prepareRoot(fileRoot), (error) => /^JOBS_DIR_/.test(error.code));
  const real = path.join(dir, "real");
  fs.mkdirSync(real);
  assert.throws(() => jobs.checkRoot(real, { platform: "linux", uid: 4242 }), { code: "JOBS_DIR_UNSAFE" });
  assert.equal(jobs.checkRoot(real), real);
  await assert.rejects(new DemoTool({ store: fx.memoryStore(), production: true }).startExport("x", {}), { code: "JOBS_DIR_UNAVAILABLE" });
});

test("чистка старых папок: ссылка внутри корня снимается сама, папка задания со ссылкой наружу удаляется без цели", (t) => {
  const dir = tempDir(t);
  const root = path.join(dir, jobs.JOBS_DIR);
  fs.mkdirSync(root);
  const outside = path.join(dir, "outside");
  fs.mkdirSync(path.join(outside, "deep"), { recursive: true });
  fs.writeFileSync(path.join(outside, "deep", "keep.txt"), "x");
  fs.symlinkSync(outside, path.join(root, "old-link"), "junction");
  const job = path.join(root, "old-job");
  fs.mkdirSync(job);
  fs.writeFileSync(path.join(job, "frame.tga"), "x");
  fs.symlinkSync(outside, path.join(job, "inner-link"), "junction");
  assert.equal(jobs.cleanupStale(root, Date.now() + 3 * 24 * 3600 * 1000), 2);
  assert.deepEqual(fs.readdirSync(root), []);
  assert.ok(fs.existsSync(path.join(outside, "deep", "keep.txt")), "цель ссылок цела");
});

test("имя итога: без запрещённых символов и имён устройств, интервал, скорость, Discord", () => {
  assert.equal(jobs.safeBaseName('a<b>:c"d/e\\f|g?h*i\u0001'), "a_b__c_d_e_f_g_h_i_");
  assert.equal(jobs.safeBaseName("con"), "_con");
  assert.equal(jobs.safeBaseName("CONIN$"), "_CONIN$");
  assert.equal(jobs.safeBaseName("com¹.mp4"), "_com¹.mp4");
  assert.equal(jobs.safeBaseName("run.  "), "run");
  assert.equal(jobs.safeBaseName(""), "demo");
  assert.equal(jobs.safeBaseName("x".repeat(300)).length, 120);
  const item = { name: "myrun1", duration: 100 };
  assert.equal(jobs.outputBaseName(item, { range: { from: 0, to: 100 }, speed: 1, preset: "quality" }), "myrun1");
  assert.equal(jobs.outputBaseName({ name: "c1a0.dem", duration: 100 }, { range: { from: 65, to: 90.5 }, speed: 0.5, preset: "discord" }),
    "c1a0_01-05_01-30_x0.5_discord");
});

test("чистка: старые папки заданий удаляются только внутри корня; при старте — все папки, файлы-маркеры остаются", (t) => {
  const dir = tempDir(t);
  const root = path.join(dir, jobs.JOBS_DIR);
  fs.mkdirSync(path.join(root, "old", "inner"), { recursive: true });
  fs.writeFileSync(path.join(root, "old", "inner", "frame.tga"), "x");
  fs.mkdirSync(path.join(root, "fresh"));
  fs.writeFileSync(path.join(root, "ghosting-render.settings.reg"), "x");
  const outside = path.join(dir, "outside-old");
  fs.mkdirSync(outside);
  const past = new Date(Date.now() - 2 * 24 * 3600 * 1000);
  fs.utimesSync(path.join(root, "old"), past, past);
  fs.utimesSync(path.join(root, "ghosting-render.settings.reg"), past, past);
  fs.utimesSync(outside, past, past);
  assert.equal(jobs.cleanupStale(root), 1);
  assert.deepEqual(fs.readdirSync(root).sort(), ["fresh", "ghosting-render.settings.reg"]);
  // Старт лаунчера: заданий ещё нет — удаляются все папки, файлы корня (снимок реестра) — нет.
  new DemoTool({ store: fx.memoryStore(), jobsRoot: root, production: true, ghostingAdapter: { recover() {}, isActive: () => false, disposeSync() {} } });
  assert.deepEqual(fs.readdirSync(root), ["ghosting-render.settings.reg"]);
  assert.ok(fs.existsSync(outside));
  assert.equal(jobs.cleanupStale(path.join(dir, "missing")), 0);
});

test("подставной адаптер и подмена FFmpeg — только вне production-сборки", (t) => {
  const dir = tempDir(t);
  const env = { HLSR_DEMO_FAKE_RENDER: "1", [ffmpeg.ENV_OVERRIDE]: dir };
  assert.deepEqual(adapters.defaultAdapters({ env, production: true }).map((adapter) => adapter.id), ["goldsrc", "source"]);
  assert.deepEqual(adapters.defaultAdapters({ env, production: false }).map((adapter) => adapter.id), ["fake", "goldsrc", "source"]);
  assert.equal(ffmpeg.locate({ platform: "win32", env, production: true, toolDir: null }), null);
  assert.deepEqual(ffmpeg.locate({ platform: "win32", env, production: false }), {
    ffmpeg: path.join(dir, "ffmpeg.exe"), ffprobe: path.join(dir, "ffprobe.exe"), source: "env",
  });
  assert.deepEqual(ffmpeg.locate({ platform: "linux", env: {}, production: true }), { ffmpeg: "ffmpeg", ffprobe: "ffprobe", source: "path" });
  fs.writeFileSync(path.join(dir, "ffmpeg.exe"), "");
  assert.equal(ffmpeg.locate({ platform: "win32", env: {}, production: true, toolDir: dir }).source, "tool");
  const pick = adapters.pickAdapter({ game: "hl1", duration: 5 }, adapters.defaultAdapters({ env: {}, production: true }));
  assert.equal(pick.code, "RENDER_NOT_READY");
  assert.equal(adapters.pickAdapter({ game: "other", duration: 5 }, adapters.defaultAdapters({ env: {}, production: true })).code, "DEMO_UNSUPPORTED");
});

// Окно лаунчера для плеера: подставное, как прозрачное окно без рамки на
// Windows (Electron 45, проверено): полный экран — через размер окна,
// isFullScreen() всегда false; enter/leave-full-screen приходят только на
// setFullScreen; свёрнутое или спрятанное окно setBounds не принимает.
function fakeWindow() {
  const { EventEmitter } = require("node:events");
  const win = new EventEmitter();
  const state = { bounds: { x: 100, y: 80, width: 1100, height: 700 }, resizable: false, fullscreenable: false, emulated: false,
    minimized: false, visible: true };
  win.state = state;
  win.webContents = Object.assign(new EventEmitter(), { isDestroyed: () => false });
  const move = (bounds) => {
    state.bounds = Object.assign({}, bounds);
    win.emit("resize");
    win.emit("move");
  };
  Object.assign(win, {
    isDestroyed: () => false,
    getBounds: () => Object.assign({}, state.bounds),
    setBounds: (bounds) => { if (!state.minimized && state.visible) move(bounds); },
    isResizable: () => state.resizable,
    setResizable: (value) => { state.resizable = value; },
    setFullScreenable: (value) => { state.fullscreenable = value; },
    isFullScreen: () => false,
    isMinimized: () => state.minimized,
    isVisible: () => state.visible,
    setFullScreen(value) {
      if (value && (!state.fullscreenable || !state.resizable)) return;
      if (value === state.emulated) return;
      state.emulated = value;
      if (value) {
        state.before = Object.assign({}, state.bounds);
        move({ x: 0, y: 0, width: 2560, height: 1440 });
      } else if (!state.minimized && state.visible) move(state.before);
      win.emit(value ? "enter-full-screen" : "leave-full-screen");
    },
    minimize() { state.minimized = true; win.emit("minimize"); },
    restore() { state.minimized = false; win.emit("restore"); },
    hide() { state.visible = false; win.emit("hide"); },
    show() { state.visible = true; win.emit("show"); },
  });
  return win;
}

test("плеер на весь экран (прозрачное окно лаунчера): ответ — своё состояние, выход по запросу, Esc, сворачиванию, скрытию, смене размера", () => {
  const { createPlayerFullscreen } = require("../src/main/demo-tool/player-fullscreen");
  const events = [];
  const fullscreen = createPlayerFullscreen((value) => events.push(value));
  const initial = { x: 100, y: 80, width: 1100, height: 700 };
  const back = (win, label) => {
    assert.deepEqual(win.state.bounds, initial, label);
    assert.equal(win.state.resizable, false, label);
    assert.equal(win.state.fullscreenable, false, label);
    assert.equal(win.state.emulated, false, label);
  };

  const win = fakeWindow();
  assert.equal(fullscreen.set(win, true), true, "isFullScreen() врёт false — ответ по своему состоянию");
  assert.deepEqual(win.state.bounds, { x: 0, y: 0, width: 2560, height: 1440 });
  assert.equal(fullscreen.set(win, true), true, "повторный запрос ничего не ломает");
  assert.equal(fullscreen.set(win, false), false);
  back(win, "по запросу");
  assert.deepEqual(events, [], "выход по запросу интерфейса — без события");

  const cases = [
    ["Esc", (w) => w.webContents.emit("before-input-event", {}, { type: "keyDown", key: "Escape" })],
    ["свернули и развернули", (w) => { w.minimize(); assert.deepEqual(events, [false], "событие сразу"); assert.equal(w.state.emulated, true, "свёрнутое не трогаем"); w.restore(); }],
    ["спрятали и показали", (w) => { w.hide(); assert.deepEqual(events, [false]); w.show(); }],
    ["система сменила размер", (w) => w.setBounds({ x: 10, y: 10, width: 1200, height: 700 })],
    ["setFullScreen(false) извне", (w) => w.setFullScreen(false)],
  ];
  for (const [label, leave] of cases) {
    const other = fakeWindow();
    events.length = 0;
    assert.equal(fullscreen.set(other, true), true, label);
    other.webContents.emit("before-input-event", {}, { type: "keyUp", key: "Escape" });
    other.webContents.emit("before-input-event", {}, { type: "keyDown", key: "Space" });
    assert.equal(other.state.emulated, true, `${label}: другие клавиши не выводят из полного экрана`);
    leave(other);
    back(other, label);
    assert.deepEqual(events, [false], label);
    assert.equal(other.listenerCount("leave-full-screen") + other.listenerCount("resize") + other.listenerCount("restore") +
      other.listenerCount("show") + other.webContents.listenerCount("before-input-event"), 0, `${label}: обработчики сняты`);
    assert.equal(fullscreen.active(), false, label);
  }
  assert.equal(fullscreen.set(null, true), false);
});
test("полный экран лаунчера (кнопка в шапке): без Esc, масштаб, плеер не забирает чужой полный экран", () => {
  const { createPlayerFullscreen } = require("../src/main/demo-tool/player-fullscreen");
  const zoom = [];
  const launcherEvents = [];
  const launcher = createPlayerFullscreen((value) => launcherEvents.push(value), {
    escape: false,
    onEnter: () => zoom.push("enter"),
    onLeave: () => zoom.push("leave"),
  });
  const player = createPlayerFullscreen(() => {});
  const win = fakeWindow();
  assert.equal(launcher.set(win, true), true);
  assert.deepEqual(zoom, ["enter"]);
  win.webContents.emit("before-input-event", {}, { type: "keyDown", key: "Escape" });
  assert.equal(launcher.active(), true, "Esc закрывает окна интерфейса, а не полный экран");
  assert.equal(win.webContents.listenerCount("before-input-event"), 0);
  assert.equal(player.set(win, true), true, "окно уже на весь экран — плееру хватает");
  assert.equal(player.active(), false, "плеер в полный экран не входил");
  assert.equal(player.set(win, false), false);
  assert.equal(win.state.emulated, true, "выход плеера не трогает полный экран лаунчера");
  win.minimize();
  assert.deepEqual(launcherEvents, [false]);
  win.restore();
  assert.deepEqual(win.state.bounds, { x: 100, y: 80, width: 1100, height: 700 });
  assert.deepEqual(zoom, ["enter", "leave"]);
  assert.equal(player.set(win, true), true, "окно свободно — плеер входит сам");
  assert.equal(player.active(), true);
  assert.equal(player.set(win, false), false);
});
test("полный экран лаунчера на рабочую область: панель задач видна, выход по смене размера", () => {
  const { createPlayerFullscreen } = require("../src/main/demo-tool/player-fullscreen");
  const events = [];
  const area = { x: 0, y: 0, width: 2560, height: 1392 };
  const launcher = createPlayerFullscreen((value) => events.push(value), { escape: false, bounds: () => area });
  const win = fakeWindow();
  assert.equal(launcher.set(win, true), true);
  assert.deepEqual(win.state.bounds, area);
  assert.equal(win.state.emulated, false, "системного полного экрана нет");
  assert.equal(win.state.fullscreenable, false);
  win.setBounds({ x: 10, y: 10, width: 1600, height: 900 });
  assert.deepEqual(events, [false], "окно сдвинули — режим снят");
  assert.deepEqual(win.state.bounds, { x: 100, y: 80, width: 1100, height: 700 });
  assert.equal(win.state.resizable, false);
  assert.equal(launcher.set(win, true), true);
  assert.equal(launcher.set(win, false), false);
  assert.deepEqual(win.state.bounds, { x: 100, y: 80, width: 1100, height: 700 });
});
test("demo-player-fullscreen: только окно лаунчера, ответ — состояние", async () => {
  const handlers = new Map();
  const tool = new DemoTool({ store: fx.memoryStore(), ghostingAdapter: { isActive: () => false, recover() {}, disposeSync() {} } });
  const win = fakeWindow();
  DemoTool.bind({ handle: (channel, fn) => handlers.set(channel, fn) }, tool, (event) => event.sender === "launcher", () => win);
  const handler = handlers.get("demo-player-fullscreen");
  assert.deepEqual(await handler({ sender: "other" }, true), { status: "error", code: "UNTRUSTED" });
  assert.equal(win.state.emulated, false);
  assert.equal(await handler({ sender: "launcher" }, true), true);
  assert.equal(await handler({ sender: "launcher" }, false), false);
  assert.equal(win.state.fullscreenable, false);
  await handler({ sender: "launcher" }, true);
  tool.dispose();
  assert.equal(win.state.emulated, false, "выход лаунчера возвращает окно");
});
test("надпись проверки данных: только у адаптера, который её умеет; запоминается; предпросмотр берёт сохранённую", { timeout: 120000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const seen = [];
  const capture = (caps) => {
    const base = adapters.fakeAdapter({ ffmpeg: ff.ffmpeg });
    return Object.assign({}, base, {
      capabilities: Object.assign({}, base.capabilities, caps),
      render: (args) => {
        seen.push(args.integrity);
        return base.render(args);
      },
    });
  };
  const dir = tempDir(t);
  const install = path.join(dir, "game");
  fx.writeGhostingRun(path.join(install, "ghostingmod", "demos"), "myrun1", [{ map: "d1_trainstation_01", duration: 3 }]);
  const make = (caps) => new DemoTool({
    store: fx.memoryStore(),
    games: () => [{ gameId: "ghosting-renewed", installPath: install, config: localConfigOf("ghosting-renewed") }],
    jobsRoot: path.join(dir, "jobs"),
    videosDir: path.join(dir, "Videos"),
    env: { [ffmpeg.ENV_OVERRIDE]: ff.dir },
    production: false,
    adapters: [capture(caps)],
  });

  const tool = make({ integrity: true });
  const id = tool.list().items[0].id;
  assert.equal((await tool.options(id)).capabilities.integrity, true);
  assert.equal(tool.savedSettings().integrity, false, "по умолчанию выключена");
  await tool.startExport(id, { preset: "quality", height: 720, fps: 30, encoder: "cpu", integrity: true });
  assert.ok((await tool.lastJob).result);
  assert.equal(tool.savedSettings().integrity, true, "запомнилась вместе с настройками экспорта");
  await tool.startPreview(id, {});
  assert.ok((await tool.lastJob).result);
  tool.clearPreview();
  assert.deepEqual(seen, [true, true], "экспорт и предпросмотр");

  // Адаптер без этой возможности: переключателя нет, до рендера не доходит.
  seen.length = 0;
  const plain = make({ integrity: false });
  assert.equal((await plain.options(id)).capabilities.integrity, false);
  await plain.startExport(id, { preset: "quality", height: 720, fps: 30, encoder: "cpu", integrity: true });
  assert.ok((await plain.lastJob).result);
  assert.deepEqual(seen, [false]);
});
test("готовое видео удалили во вкладке «Отрендеренные»: та же демка снова даёт предпросмотр и экспорт", { timeout: 180000 }, async (t) => {
  if (!ff) return t.skip(NO_FFMPEG);
  const trash = [];
  const revealed = [];
  const shell = {
    trashItem: async (file) => {
      trash.push(path.basename(file));
      fs.rmSync(file);
    },
    showItemInFolder: (file) => revealed.push(file),
  };
  const { tool, id, dir } = setup(t, { shell });
  const out = path.join(dir, "Videos", "HLSR");
  await tool.startExport(id, { preset: "quality", height: 720, fps: 30, encoder: "cpu", range: { from: 1, to: 3 } });
  const first = await tool.lastJob;
  assert.equal(first.kind, "export");
  const name = first.result.fileName;
  const exported = await tool.media.handle(new Request(first.result.mediaUrl));
  assert.equal(exported.status, 200);
  await exported.body.cancel();

  // Вкладка «Отрендеренные»: видео есть, источник найден, «Открыть папку» — только файл папки вывода.
  const opened = tool.renderedOpen(name);
  assert.equal(opened.sourceId, id);
  assert.deepEqual(opened.video.range, { from: 1, to: 3 });
  assert.deepEqual(tool.renderedReveal(name), { status: "ok" });
  assert.deepEqual(revealed, [path.join(out, name)]);
  assert.throws(() => tool.renderedReveal("..\\" + name), { code: "PATH_INVALID" });
  assert.throws(() => tool.renderedReveal("missing.mp4"), { code: "RESULT_NOT_FOUND" });

  // Удаление: файл и его метки в Корзине, итог экспорта больше не отдаётся (его адрес — 404).
  const answer = await tool.renderedDelete(name);
  assert.equal(answer.deleted, path.join(out, name));
  assert.deepEqual(trash.sort(), [name, name.replace(/\.mp4$/, ".timeline.json")].sort());
  assert.equal((await tool.media.handle(new Request(first.result.mediaUrl))).status, 404);
  assert.throws(() => tool.result(first.jobId), { code: "RESULT_NOT_FOUND" });

  // Та же демка: предпросмотр и экспорт работают как раньше.
  await tool.startPreview(id, { range: { from: 0, to: 2 } });
  const preview = await tool.lastJob;
  assert.equal(preview.kind, "preview");
  assert.ok(preview.result && preview.result.mediaUrl, JSON.stringify(preview.error || null));
  const previewMedia = await tool.media.handle(new Request(preview.result.mediaUrl));
  assert.equal(previewMedia.status, 200);
  await previewMedia.body.cancel();
  await tool.startExport(id, { preset: "quality", height: 720, fps: 30, encoder: "cpu", range: { from: 1, to: 3 } });
  const again = await tool.lastJob;
  assert.ok(again.result, JSON.stringify(again.error || null));
  assert.equal(again.result.fileName, name, "имя освободилось — то же");
  assert.ok(fs.existsSync(path.join(out, name)));
  assert.equal((await tool.media.handle(new Request(again.result.mediaUrl))).status, 200);
});
