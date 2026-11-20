"use strict";

const path = require("path");

const { DemoError } = require("./errors");
const planner = require("./planner");
const ffmpeg = require("./ffmpeg");

// Адаптер рендера даёт сырьё в папке задания, кодирование общее (jobs.js):
// {
//   id, engine,
//   capabilities: { maxHeight, fps: [..], keyhud: boolean, audio: boolean, integrity?: boolean (надпись проверки данных демки),
//     hud?, ownConfig?, ownHud?: boolean (можно выключить HUD игры, взять стандартный конфиг, стандартный HUD) },
//   canRender(item) -> { ok: true } | { ok: false, code },
//   async render({ item, segments, range, fps, width, height, audio, keyhud, integrity, jobDir, ffmpegPath, ffprobePath, signal, onProgress })
//     -> { video: <файл в jobDir> | { pattern, fps, startNumber }, audio: <файл со звуком в jobDir> | null,
//          offset: <время демо первого кадра, по умолчанию range.from>,
//          timeline: [{ t: <время демо>, type: "map"|"segment"|"saveload"|"timer", label }],
//          cuts: [{ from, to }] — вырезанные из материала участки (загрузки), время демо,
//          overlay: <имя ASS в jobDir, время материала> | null — KeyHUD, который рисует лаунчер (HL1) }
// }
// onProgress({ fraction: 0..1, segment: { index, count } | null, message }).
// Процессы адаптер гасит по своим PID, когда срабатывает signal.

const FAKE_FLAG = "HLSR_DEMO_FAKE_RENDER";
const PENDING_CAPS = Object.freeze({ maxHeight: 2160, fps: [30, 60, 120], keyhud: false, audio: true });

// Настоящих адаптеров ещё нет: игра показывается, рендер — следующим шагом.
function pendingAdapter(id, engine, game) {
  return {
    id,
    engine,
    capabilities: PENDING_CAPS,
    canRender(item) {
      return { ok: false, code: item && item.game === game ? "RENDER_NOT_READY" : "DEMO_UNSUPPORTED" };
    },
    async render() {
      throw new DemoError("RENDER_NOT_READY");
    },
  };
}

// Подставной адаптер (тесты и dev-флаг): испытательная таблица FFmpeg с
// номером кадра и синус в WAV длительностью отрезка, метки из источника.
function fakeAdapter(options) {
  const opts = options || {};
  const runFn = opts.runFn || ffmpeg.run;
  return {
    id: "fake",
    engine: "any",
    capabilities: Object.freeze({ maxHeight: 2160, fps: [30, 60, 120], keyhud: false, audio: true }),
    canRender(item) {
      return item && item.duration > 0 ? { ok: true } : { ok: false, code: "DURATION_UNKNOWN" };
    },
    async render({ item, range, fps, width, height, audio, jobDir, signal, onProgress }) {
      const cuts = planner.normalizeCuts(opts.cuts).filter((cut) => cut.to > range.from && cut.from < range.to);
      const length = planner.clipDuration({ from: range.from, to: range.to, speed: 1, cuts });
      const video = path.join(jobDir, "fake.mkv");
      const wav = audio ? path.join(jobDir, "fake.wav") : null;
      const args = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats",
        "-f", "lavfi", "-i", `testsrc=size=${width}x${height}:rate=${fps}:duration=${length}`];
      if (wav) args.push("-f", "lavfi", "-i", `sine=frequency=440:sample_rate=48000:duration=${length}`);
      args.push("-map", "0:v", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "12", "-pix_fmt", "yuv420p", video);
      if (wav) args.push("-map", "1:a", "-c:a", "pcm_s16le", wav);
      await runFn(opts.ffmpeg, args, {
        signal,
        onSpawn: opts.onSpawn,
        onProgress: (seconds) => onProgress && onProgress({ fraction: Math.min(1, seconds / length), segment: null, message: null }),
      });
      const timeline = [{ t: range.from, type: "map", label: item.map || "fake" }]
        .concat(item.markers || [])
        .concat([{ t: range.from + (range.to - range.from) / 2, type: "timer", label: "fake" }]);
      return { video, audio: wav, offset: range.from, timeline, cuts };
    },
  };
}

// Подставной адаптер не попадает в обычную сборку (NODE_ENV=production).
// ghosting и goldsrc — по одному экземпляру на лаунчер (у них состояние:
// идёт ли рендер, маркер возврата реестра); без них — заглушки.
function defaultAdapters({ env, production, ffmpegPath, ghosting, goldsrc }) {
  const list = [];
  if (!production && env && env[FAKE_FLAG] === "1") list.push(fakeAdapter({ ffmpeg: ffmpegPath }));
  list.push(goldsrc || pendingAdapter("goldsrc", "goldsrc", "hl1"), ghosting || pendingAdapter("source", "source", "ghosting"));
  return list;
}

// Первый адаптер, готовый рендерить; иначе самый понятный отказ.
function pickAdapter(item, adapters) {
  let refusal = null;
  for (const adapter of adapters || []) {
    const verdict = adapter.canRender(item) || { ok: false, code: "DEMO_UNSUPPORTED" };
    if (verdict.ok) return { adapter, ok: true, code: null };
    if (!refusal || (refusal.code === "DEMO_UNSUPPORTED" && verdict.code !== "DEMO_UNSUPPORTED"))
      refusal = { adapter, ok: false, code: verdict.code };
  }
  return refusal || { adapter: null, ok: false, code: "DEMO_UNSUPPORTED" };
}

module.exports = { FAKE_FLAG, PENDING_CAPS, pendingAdapter, fakeAdapter, defaultAdapters, pickAdapter };
