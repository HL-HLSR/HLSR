"use strict";

// Оценка времени задания «Демо»: запуск игры + кадры игры + кодирование итога.
//
// Скорости по умолчанию — замер 11.11 на стенде (Ghosting, 1280×720, 30 к/с):
//   -ghrender: ран 0:08 — 17–19 с (почти всё — запуск игры, ~12 с), ран 2:24 — 103 с,
//     из них игра рисует 4291 кадр за 96 с (~45 к/с), кодирование — несколько секунд;
//   старый путь startdemos: тот же ран 2:24 — две пачки 76 + 16 с (~63 к/с без запусков).
// HL1 (bxt-rs) не замерен — осторожная оценка; прошлые задания её поправят (learn).
// Другое разрешение: скорость игры — обратно числу пикселей в степени 0,6 (кадр TGA
// растёт с площадью, отрисовка — медленнее), кодирование — обратно числу пикселей.

const BASE_PIXELS = 1280 * 720;
const RENDER_EXPONENT = 0.6;
const PROFILES = Object.freeze({
  "ghosting-ghrender": { load: 12, rate: 45, perSegment: 0 },
  "ghosting-startdemos": { load: 12, rate: 63, perSegment: 0 },
  hl1: { load: 10, rate: 60, perSegment: 1 },
  other: { load: 10, rate: 45, perSegment: 0 },
});
// Кодирование итога, выходных кадров в секунду на 1280×720: предпросмотр (veryfast/p2 —
// замер ~600), «Лучшее качество» (x264 slow CRF 18 / NVENC p5), Discord (x264 в два прохода).
const ENCODE = Object.freeze({ preview: 600, "quality-gpu": 500, "quality-cpu": 90, "discord-gpu": 500, "discord-cpu": 100 });
// Предпросмотр дольше этого — окно с советом сразу отрендерить видео.
const PREVIEW_PROMPT_SECONDS = 120;
// Свой замер учитывается от этого числа кадров; вес прошлых — не больше CAP кадров.
const MIN_FRAMES = 300;
const CAP_FRAMES = 30000;
const RATE_LIMITS = [1, 5000];

const pixels = (width, height) => Math.max(1, width * height);
const clampRate = (rate) => Math.min(RATE_LIMITS[1], Math.max(RATE_LIMITS[0], rate));
const encodeKind = (preset, gpu) => (preset === "preview" ? "preview" : `${preset === "discord" ? "discord" : "quality"}-${gpu ? "gpu" : "cpu"}`);

// Скорость из своих замеров: точно это разрешение, иначе ближайшее того же вида — пересчётом
// по пикселям (высота -> 16:9). null — замеров нет.
function learnedRate(table, prefix, height, exponent) {
  const own = table && table[`${prefix}@${height}`];
  if (own && own.rate > 0) return own.rate;
  let best = null;
  for (const [key, value] of Object.entries(table || {})) {
    const at = key.lastIndexOf("@");
    if (key.slice(0, at) !== prefix || !(value && value.rate > 0)) continue;
    const other = Number(key.slice(at + 1));
    if (!(other > 0)) continue;
    if (!best || Math.abs(other - height) < Math.abs(best.height - height)) best = { height: other, rate: value.rate };
  }
  return best ? best.rate * Math.pow((best.height * best.height) / (height * height), exponent) : null;
}

function renderRate(profile, width, height, learned) {
  const own = learnedRate(learned && learned.render, profile, height, RENDER_EXPONENT);
  if (own) return clampRate(own);
  const base = PROFILES[profile] || PROFILES.other;
  return clampRate(base.rate / Math.pow(pixels(width, height) / BASE_PIXELS, RENDER_EXPONENT));
}

function encodeRate(kind, width, height, learned) {
  const own = learnedRate(learned && learned.encode, kind, height, 1);
  if (own) return clampRate(own);
  return clampRate((ENCODE[kind] || ENCODE["quality-cpu"]) / (pixels(width, height) / BASE_PIXELS));
}

// input: { profile, launches, segments, seconds (длина отрезка в демке), fps, width, height,
// outputSeconds (длина видео), preset, gpu }. -> { seconds, parts: { load, render, encode }, learned }.
function estimate(input, learned) {
  const base = PROFILES[input.profile] || PROFILES.other;
  const load = base.load * Math.max(1, input.launches || 1) + base.perSegment * Math.max(0, (input.segments || 1) - 1);
  const frames = Math.max(0, Number(input.seconds) || 0) * input.fps;
  const render = frames / renderRate(input.profile, input.width, input.height, learned);
  const kind = encodeKind(input.preset, input.gpu);
  // Discord кодируется в два прохода только на процессоре: учтено в его скорости.
  const encode = (Math.max(0, Number(input.outputSeconds) || 0) * input.fps) / encodeRate(kind, input.width, input.height, learned);
  const own = !!((learned && learned.render && learned.render[`${input.profile}@${input.height}`]) ||
    (learned && learned.encode && learned.encode[`${kind}@${input.height}`]));
  return { seconds: Math.round(load + render + encode), parts: { load: Math.round(load), render: Math.round(render), encode: Math.round(encode) }, learned: own };
}

function merge(previous, rate, frames) {
  const weight = previous && previous.rate > 0 ? Math.min(CAP_FRAMES, previous.frames || 0) : 0;
  const value = weight ? (previous.rate * weight + rate * frames) / (weight + frames) : rate;
  return { rate: Math.round(clampRate(value) * 100) / 100, frames: Math.min(CAP_FRAMES, weight + frames) };
}

// Замер готового задания (jobs.js stats) -> новая таблица скоростей. Запуск игры берётся из
// профиля по умолчанию: один замер не разделяет его и кадры, а короткий ран почти весь — запуск.
function learn(learned, stats) {
  const out = { render: Object.assign({}, learned && learned.render), encode: Object.assign({}, learned && learned.encode) };
  if (!stats || !(stats.height > 0)) return out;
  const base = PROFILES[stats.profile];
  if (base && stats.frames >= MIN_FRAMES && stats.renderSeconds > 0) {
    const load = base.load * Math.max(1, stats.launches || 1) + base.perSegment * Math.max(0, (stats.segments || 1) - 1);
    const busy = Math.max(stats.renderSeconds - load, stats.frames / RATE_LIMITS[1]);
    const key = `${stats.profile}@${stats.height}`;
    out.render[key] = merge(out.render[key], stats.frames / busy, stats.frames);
  }
  if (stats.preset && stats.outputFrames >= MIN_FRAMES && stats.encodeSeconds > 0.5) {
    const key = `${encodeKind(stats.preset, stats.gpu)}@${stats.height}`;
    out.encode[key] = merge(out.encode[key], stats.outputFrames / stats.encodeSeconds, stats.outputFrames);
  }
  return out;
}

module.exports = { PROFILES, ENCODE, PREVIEW_PROMPT_SECONDS, MIN_FRAMES, encodeKind, renderRate, encodeRate, estimate, learn };
