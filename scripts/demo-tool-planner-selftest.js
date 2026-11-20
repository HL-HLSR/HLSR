"use strict";

// «Демо»: планировщик экспорта — сочетания, Discord-битрейт и повтор,
// цепочка atempo, пересчёт таймлайна, аргументы FFmpeg.
const assert = require("node:assert/strict");
const test = require("node:test");

const planner = require("../src/main/demo-tool/planner");

const caps = { maxHeight: 2160, fps: [30, 60, 120], keyhud: false, audio: true };

test("настройки по умолчанию: 1080p60, весь ран, скорость 1", () => {
  const settings = planner.normalizeSettings({}, 125.5);
  assert.deepEqual(settings, {
    preset: "quality", height: 1080, width: 1920, fps: 60, audio: true, keyhud: false, integrity: false, encoder: "auto", speed: 1,
    hud: true, ownConfig: true, ownHud: true, range: { from: 0, to: 125.5 },
  });
  // HUD, конфиг и HUD игрока: по умолчанию как у игрока, выключает только явное false.
  const view = planner.normalizeSettings({ hud: false, ownConfig: false, ownHud: "no" }, 10);
  assert.deepEqual([view.hud, view.ownConfig, view.ownHud], [false, false, false]);
  assert.deepEqual(planner.VIEW_KEYS, ["keyhud", "hud", "ownConfig", "ownHud"]);
  // Надпись «GH DEMO DATA: INTACT»: только явное true.
  assert.equal(planner.normalizeSettings({ integrity: true }, 10).integrity, true);
  assert.equal(planner.normalizeSettings({ integrity: "yes" }, 10).integrity, false);
});

test("настройки: неверные значения — SETTINGS_INVALID / RANGE_INVALID / DURATION_UNKNOWN", () => {
  assert.throws(() => planner.normalizeSettings({ height: 900 }, 10), { code: "SETTINGS_INVALID", details: { field: "height" } });
  assert.throws(() => planner.normalizeSettings({ fps: 144 }, 10), { code: "SETTINGS_INVALID" });
  assert.throws(() => planner.normalizeSettings({ speed: 5 }, 10), { code: "SETTINGS_INVALID" });
  assert.throws(() => planner.normalizeSettings({ speed: 0.2 }, 10), { code: "SETTINGS_INVALID" });
  assert.throws(() => planner.normalizeSettings({ encoder: "nvenc" }, 10), { code: "SETTINGS_INVALID" });
  // Пресет предпросмотра — только изнутри startPreview, не из окна экспорта.
  assert.throws(() => planner.normalizeSettings({ preset: "preview" }, 10), { code: "SETTINGS_INVALID", details: { field: "preset" } });
  assert.equal(planner.normalizeSettings(planner.PREVIEW, 10, { allowPreview: true }).preset, "preview");
  assert.throws(() => planner.normalizeSettings({ range: { from: 5, to: 4 } }, 10), { code: "RANGE_INVALID" });
  assert.throws(() => planner.normalizeSettings({ range: { from: 0, to: 11 } }, 10), { code: "RANGE_INVALID" });
  assert.throws(() => planner.normalizeSettings({ range: { from: -1, to: 4 } }, 10), { code: "RANGE_INVALID" });
  assert.throws(() => planner.normalizeSettings({}, null), { code: "DURATION_UNKNOWN" });
  // Конец чуть дальше длительности (ползунок) прижимается.
  assert.deepEqual(planner.normalizeSettings({ range: { from: 1, to: 10.03 } }, 10).range, { from: 1, to: 10 });
});

test("сочетания: пересечение адаптера и проб кодировщика; 4K120 — только если проба на 4K прошла", () => {
  const all = planner.allowedCombos(caps, () => true);
  assert.equal(all.length, 12);
  const noFourK120 = planner.allowedCombos(caps, (height, fps) => !(height === 2160 && fps === 120));
  assert.equal(noFourK120.length, 11);
  assert.ok(!noFourK120.some((combo) => combo.height === 2160 && combo.fps === 120));
  const small = planner.allowedCombos({ maxHeight: 1080, fps: [30, 60] }, () => true);
  assert.deepEqual(small.map((combo) => `${combo.width}x${combo.height}@${combo.fps}`),
    ["1280x720@30", "1280x720@60", "1920x1080@30", "1920x1080@60"]);
});

test("atempo: множители в [0.5, 2], произведение равно скорости", () => {
  const cases = { 1: [1], 2: [2], 4: [2, 2], 3: [2, 1.5], 0.5: [0.5], 0.25: [0.5, 0.5], 0.3: [0.5, 0.6], 1.25: [1.25] };
  for (const [speed, chain] of Object.entries(cases)) {
    assert.deepEqual(planner.atempoChain(Number(speed)), chain, speed);
    assert.ok(chain.every((factor) => factor >= 0.5 && factor <= 2));
    assert.ok(Math.abs(chain.reduce((a, b) => a * b, 1) - Number(speed)) < 1e-6);
  }
  assert.equal(planner.audioFilter(4), "asetpts=PTS-STARTPTS,atempo=2,atempo=2,aresample=48000,apad");
  assert.equal(planner.audioFilter(1), "asetpts=PTS-STARTPTS,aresample=48000,apad");
});

test("пересчёт таймлайна: интервал, скорость, вырезанные загрузки", () => {
  const markers = [
    { t: 1, type: "map", label: "c1a0" },
    { t: 10, type: "segment", label: "2" },
    { t: 20, type: "saveload", label: "3" },
    { t: 30, type: "timer", label: "end" },
    { t: 12, type: "bogus", label: "x" },
    { t: NaN, type: "map", label: "y" },
  ];
  assert.deepEqual(planner.mapTimeline(markers, { from: 5, to: 25, speed: 2 }), [
    { t: 2.5, type: "segment", label: "2", sourceTime: 10 },
    { t: 7.5, type: "saveload", label: "3", sourceTime: 20 },
  ]);
  // Загрузка 12–16 вырезана: метка 20 сдвигается на 4 с раньше.
  assert.deepEqual(planner.mapTimeline(markers, { from: 5, to: 25, speed: 1, cuts: [{ from: 12, to: 16 }] }), [
    { t: 5, type: "segment", label: "2", sourceTime: 10 },
    { t: 11, type: "saveload", label: "3", sourceTime: 20 },
  ]);
  // Метка внутри вырезанного участка встаёт на стык.
  assert.deepEqual(planner.mapTimeline([{ t: 14, type: "map", label: "c1a1" }], { from: 5, to: 25, speed: 1, cuts: [{ from: 12, to: 16 }] }),
    [{ t: 7, type: "map", label: "c1a1", sourceTime: 14 }]);
  assert.equal(planner.clipDuration({ from: 5, to: 25, speed: 0.5 }), 40);
  assert.equal(planner.clipDuration({ from: 5, to: 25, speed: 1, cuts: [{ from: 0, to: 6 }, { from: 12, to: 16 }, { from: 15, to: 17 }] }), 14);
  assert.deepEqual(planner.normalizeCuts([{ from: 15, to: 17 }, { from: 12, to: 16 }, { from: 3, to: 2 }]), [{ from: 12, to: 17 }]);
});

test("Discord: битрейт по длительности с запасом под звук и контейнер", () => {
  const settings = planner.normalizeSettings({ preset: "discord", height: 720, fps: 30 }, 120);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 120 });
  const expected = Math.floor((10000000 * 0.96 * 8) / 120 - 96000);
  assert.equal(plan.videoBps, expected);
  assert.equal(plan.audioBps, 96000);
  assert.equal(plan.twoPass, true);
  assert.equal(plan.limitBytes, 10000000);
  assert.ok(plan.estimatedBytes < 10000000);
  // Без звука весь бюджет — видео; GPU — однопроходный VBR с maxrate.
  const silent = planner.planExport(Object.assign({}, settings, { audio: false }), { encoder: "h264_nvenc", outputDuration: 120 });
  assert.equal(silent.audioBps, 0);
  assert.equal(silent.videoBps, Math.floor((10000000 * 0.96 * 8) / 120));
  assert.equal(silent.twoPass, false);
  const args = planner.videoCodecArgs(silent, 0);
  assert.deepEqual(args.slice(args.indexOf("-rc"), args.indexOf("-rc") + 8), ["-rc", "vbr", "-b:v", String(silent.videoBps), "-maxrate", String(silent.videoBps), "-bufsize", String(silent.videoBps * 2)]);
});

test("Discord: слишком длинный клип — DISCORD_TOO_LONG с рекомендованной длительностью и разрешением", () => {
  const settings = planner.normalizeSettings({ preset: "discord", height: 2160, fps: 60 }, 600);
  let error;
  try {
    planner.planExport(settings, { encoder: "h264_nvenc", outputDuration: 600 });
  } catch (caught) {
    error = caught;
  }
  assert.equal(error.code, "DISCORD_TOO_LONG");
  assert.equal(error.details.reason, "bitrate-floor");
  const floor = planner.minVideoBps(3840, 2160, 60);
  assert.equal(error.details.recommendedMaxSeconds, Math.floor(((10000000 * 0.96 * 8) / (floor + 96000)) * 10) / 10);
  assert.ok(error.details.recommendedMaxSeconds < 600);
  assert.equal(error.details.suggestedHeight, null, "10 минут не влезают и в 720p60");
  // 75 с в 1440p60 не влезают (928 кбит/с < 1106), а в 1080p60 — влезают (порог 622).
  const tight = planner.normalizeSettings({ preset: "discord", height: 1440, fps: 60 }, 75);
  assert.throws(() => planner.planExport(tight, { outputDuration: 75 }), (caught) => caught.details.suggestedHeight === 1080);
});

test("Discord: повтор после превышения — битрейт с поправкой по факту, не больше двух повторов", () => {
  const settings = planner.normalizeSettings({ preset: "discord", height: 720, fps: 30 }, 60);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 60 });
  const next = planner.discordRetry(plan, 11000000, 1);
  assert.ok(next < plan.videoBps);
  // Поправка: видеочасть уменьшается в отношении нужного к фактическому, с запасом 3 %.
  const audioBytes = (96000 * 60) / 8;
  const expected = Math.floor(plan.videoBps * (((10000000 * 0.96) - audioBytes) / (11000000 - audioBytes)) * 0.97);
  assert.equal(next, expected);
  assert.throws(() => planner.discordRetry(plan, 11000000, 3), (error) =>
    error.code === "DISCORD_TOO_LONG" && error.details.reason === "retries" && error.details.recommendedMaxSeconds > 0 &&
    error.details.recommendedMaxSeconds < 60);
  // Поправка уводит ниже порога качества — сразу совет укоротить.
  assert.throws(() => planner.discordRetry(plan, 60000000, 1), (error) => error.details.reason === "bitrate-floor");
});

test("качество: CRF/CQ по кодировщику, размер — грубая оценка; предупреждения", () => {
  const settings = planner.normalizeSettings({ height: 2160, fps: 120 }, 10);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 10 });
  assert.equal(plan.mode, "quality");
  assert.equal(plan.videoBps, null);
  assert.equal(plan.audioBps, 192000);
  assert.ok(plan.estimatedBytes > 0);
  assert.deepEqual(plan.warnings, ["CPU_SLOW"]);
  assert.ok(planner.videoCodecArgs(plan, 0).join(" ").includes("-crf 18"));
  for (const encoder of ["h264_nvenc", "h264_qsv", "h264_amf"]) {
    const args = planner.videoCodecArgs(Object.assign({}, plan, { encoder }), 0);
    assert.equal(args[1], encoder);
    assert.ok(!args.includes("-b:v") || args[args.indexOf("-b:v") + 1] === "0");
  }
  assert.throws(() => planner.videoCodecArgs(Object.assign({}, plan, { encoder: "h264_mf" }), 0), { code: "ENCODER_UNAVAILABLE" });
});

test("аргументы FFmpeg: обрезка входа, скорость, двухпроходный libx264, +faststart", () => {
  const settings = planner.normalizeSettings({ preset: "discord", height: 720, fps: 30, speed: 2, range: { from: 4, to: 24 } }, 30);
  const plan = planner.planExport(settings, { encoder: "libx264", outputDuration: 10 });
  const material = { video: "C:/job/raw.mkv", audio: "C:/job/raw.wav" };
  const trim = { start: 4, duration: 20 };
  const first = planner.encodeArgs({ material, trim, plan, output: "C:/job/out.mp4", pass: 1, passlog: "C:/job/pass", nullTarget: "NUL" });
  const second = planner.encodeArgs({ material, trim, plan, output: "C:/job/out.mp4", pass: 2, passlog: "C:/job/pass", nullTarget: "NUL" });
  assert.deepEqual(first.slice(first.indexOf("-ss"), first.indexOf("-ss") + 6), ["-ss", "4", "-t", "20", "-i", "C:/job/raw.mkv"]);
  assert.ok(first.includes("-an") && !first.includes("C:/job/raw.wav"), "первый проход без звука");
  assert.deepEqual(first.slice(-3), ["-f", "null", "NUL"]);
  assert.ok(first.includes("-pass") && first[first.indexOf("-pass") + 1] === "1");
  assert.ok(second.includes("C:/job/raw.wav") && second.includes("aac"));
  assert.deepEqual(second.slice(-3), ["-movflags", "+faststart", "C:/job/out.mp4"]);
  assert.ok(second[second.indexOf("-filter:v") + 1].startsWith("setpts=(PTS-STARTPTS)/2,fps=30,scale=1280:720"));
  assert.ok(second[second.indexOf("-filter:a") + 1].includes("atempo=2"));
  assert.equal(second[second.indexOf("-t", second.indexOf("-map")) + 1], "10");
  const frames = planner.encodeArgs({ material: { pattern: "C:/job/f%05d.tga", fps: 60, startNumber: 1, audio: null }, trim, plan: Object.assign({}, plan, { twoPass: false, encoder: "h264_qsv" }), output: "o.mp4", pass: 0 });
  assert.deepEqual(frames.slice(frames.indexOf("-framerate"), frames.indexOf("-framerate") + 4), ["-framerate", "60", "-start_number", "1"]);
  assert.ok(frames.includes("-an"));
  assert.ok(frames[frames.indexOf("-filter:v") + 1].endsWith("format=nv12"));
});
