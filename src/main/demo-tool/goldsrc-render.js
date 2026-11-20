"use strict";

const planner = require("./planner");
const goldsrcPersonalization = require("../game-delivery/goldsrc-personalization");
const { BUTTONS } = require("./goldsrc-demo");

// Чистая логика рендера HL1 через bxt-rs: какие сегменты рендерить, job.cfg,
// разбор qconsole.log и прогресса FFmpeg, привязка видео ко времени демки,
// метки таймлайна и KeyHUD (ASS). Процессы и файлы игры — в goldsrc.js.

// Движок режет +команды командной строки на «-» и «+» даже внутри слова:
// в именах папок задания их нет.
const RENDER_DIR = "hlsr_render";
// Сетевой протокол демок сборки HLSR WON (BUILD 3248); 48 — Steam HL1, этот движок его не играет.
const NET_PROTOCOL = 47;
// Громкость звука bxt-rs: 0,4 даёт редкие пики 0 dB (отчёт bxt-rs, раздел 3).
const CAPTURE_VOLUME = 0.3;
// Меток на ран: у настоящего рана HL1 — десятки, сотни.
const MAX_MARKERS = 1000;
const round3 = (value) => Math.round(value * 1000) / 1000;

// Сегменты набора в порядке со временем демо (сумма длительностей, как в
// library.describe). Рендерятся целиком те, что пересекают интервал; пустые
// (0 с) пропускаются — времени демо они не занимают.
function planSegments(segments, range) {
  const all = [];
  let t = 0;
  for (let i = 0; i < (segments || []).length; i++) {
    const segment = segments[i];
    const duration = Number(segment.duration) > 0 ? Number(segment.duration) : 0;
    all.push({ segment, index: i, start: round3(t), end: round3(t + duration), duration });
    t += duration;
  }
  const from = range ? range.from : 0;
  const to = range ? range.to : t;
  const picked = all.filter((item) => item.duration > 0 && item.end > from && item.start < to);
  return { picked, offset: picked.length ? picked[0].start : from };
}

// job.cfg: видео на каждый сегмент (точные границы), без кадров загрузки;
// промежуточное видео высокого качества (libx264 ultrafast CRF 14, как у
// Ghosting; звук без потерь, ALAC), итог кодирует общий слой. Аргументы FFmpeg
// bxt-rs режет по пробелам: пути относительные (папка игры) и без пробелов.
// Вид в кадре (после config.cfg, userconfig.cfg и autoexec игрока: +exec job.cfg идёт последним).
// ownHud false — значения «Персонализации» по умолчанию (цвет HUD, прицел, панели BXT); спрайты паков
// HUD — файлы игры, они остаются. hud false — HUD игры и BXT целиком и прицел (bxt_disable_hud, BXT
// внедряется в рендер вместе с bxt-rs). Свой конфиг HL1 читает всегда: стандартного нет.
function viewLines({ hud, ownHud }) {
  const lines = [];
  if (ownHud === false)
    for (const line of goldsrcPersonalization.toCfgLines({}).split(/\r?\n/)) if (line && !line.startsWith("//")) lines.push(line);
  if (hud === false) lines.push("bxt_disable_hud 1", "bxt_hud 0", "bxt_cross 0", "crosshair 0");
  return lines;
}

function jobConfig({ fps, dir, hud, ownHud }) {
  const ffmpeg = ["-c:v", "libx264", "-preset", "ultrafast", "-crf", "14", "-pix_fmt", "yuv420p", "-c:a", "alac",
    "-progress", `valve_WON/${dir}/progress.txt`, "-stats_period", "0.5"].join(" ");
  return [
    `bxt_cap_fps ${fps}`,
    `bxt_cap_volume ${CAPTURE_VOLUME}`,
    ...viewLines({ hud, ownHud }),
    "bxt_cap_skip_non_gameplay_frames 1",
    `_bxt_cap_override_ffmpeg_args "${ffmpeg}"`,
    `bxt_play_folder ${dir}`,
    `bxt_cap_separate_start valve_WON/${dir}/out`,
    "",
  ].join("\n");
}

// Командная строка игры (после Injector и полного пути к hl.exe). Всё прочее —
// в job.cfg: +команды движок режет на «-» и «+».
function gameArguments({ width, height, dir }) {
  return ["-steam", "-gl", "-32bpp", "-game", "valve_WON", "-noforcemparms", "-windowed", "-w", String(width), "-h", String(height),
    "-condebug", "+exec", `${dir}/job.cfg`];
}

// Перезапуск для сброса mute: hl.exe без внедрения. Тишину держит не +volume 0
// (config.cfg игрока исполняется позже и его перекрывает), а то, что в меню
// WON-сборки звука нет (нет media/gamestartup.mp3).
function resetArguments() {
  return ["-steam", "-gl", "-32bpp", "-game", "valve_WON", "-noforcemparms", "-windowed", "-w", "640", "-h", "480", "+volume", "0"];
}

// Строки qconsole.log (-condebug), которые важны рендеру.
const LOG_RULES = [
  { re: /^Playing demo from (?:.*[\\/])?(s\d{3})\.dem/i, kind: "playing" },
  { re: /^Recording stopped\./, kind: "stopped" },
  { re: /^Error initializing recording/i, kind: "error", code: "FFMPEG_START_FAILED" },
  { re: /^Error during recording|^Resolution has changed/i, kind: "error", code: "RENDER_FAILED" },
  // bxt-rs не нашёл движок: его команд нет.
  { re: /^Unknown command: ?(bxt_play_folder|bxt_cap_separate_start|_bxt_cap_override_ffmpeg_args|bxt_cap_)/i, kind: "error", code: "BXTRS_FAILED" },
  { re: /^Error: (no demos found|could not open directory)/i, kind: "error", code: "DEMO_OPEN_FAILED" },
  { re: /^(ERROR: )?(couldn't open|Demo file .* not found)/i, kind: "error", code: "DEMO_OPEN_FAILED" },
  { re: /demo protocol outdated|protocol .* outdated/i, kind: "error", code: "DEMO_PROTOCOL_UNSUPPORTED" },
  // Проверено на игре (карта убрана из копии): «Couldn't find map maps/c1a0d.bsp, server will
  // download the map», «Warning! File … missing during demo playback», «Cannot continue without model …».
  { re: /^Couldn't find map maps\/|^Warning! File maps\/\S+ missing during demo playback|^Cannot continue without model maps\/|Mod_NumForName: maps\/\S+ not found/i,
    kind: "error", code: "MAP_MISSING" },
  { re: /^Host_Error:|^Cannot continue without model /i, kind: "error", code: "GAME_ERROR" },
];

function parseLogLine(line) {
  const text = String(line || "").replace(/\r$/, "");
  for (const rule of LOG_RULES) {
    const match = rule.re.exec(text);
    if (!match) continue;
    if (rule.kind === "playing") return { kind: "playing", index: Number(match[1].slice(1)) };
    if (rule.kind === "error") return { kind: "error", code: rule.code, text: text.slice(0, 300) };
    return { kind: rule.kind };
  }
  return null;
}

// Последнее out_time из -progress FFmpeg (с): секунды видео текущего сегмента.
function progressSeconds(text) {
  const all = String(text || "").match(/out_time_us=(\d+)/g);
  return all ? Number(all[all.length - 1].slice(12)) / 1e6 : null;
}

// Вырезанное bxt-rs (bxt_cap_skip_non_gameplay_frames) во времени демо.
// Сколько вырезано в сегменте, известно точно: длительность демки минус
// длительность его видео (ffprobe). Где — по демке: в начале (кадры до игры)
// и на сменах карт внутри сегмента (от svc_serverinfo до возврата в игру);
// потеря делится между ними пропорционально оценке из демки (scan.startGap,
// mapChanges[].gap). Сегмент без смен карт — вся потеря в начале, точно.
function lossCuts(picked, videos, scans) {
  const cuts = [];
  picked.forEach((item, k) => {
    const loss = item.duration - (Number(videos[k]) || 0);
    if (!(loss > 0)) return;
    const scan = scans[k] || null;
    const parts = [{ at: 0, weight: Math.max(scan ? scan.startGap : 0, 1e-3) }];
    for (const change of (scan && scan.mapChanges) || []) if (change.t > 0 && change.gap > 0) parts.push({ at: change.t, weight: change.gap });
    const total = parts.reduce((sum, part) => sum + part.weight, 0);
    for (const part of parts) {
      const length = (loss * part.weight) / total;
      const from = Math.min(item.start + part.at, item.end);
      cuts.push({ from: round3(from), to: round3(Math.min(from + length, item.end)) });
    }
  });
  return planner.normalizeCuts(cuts);
}

// Метки во времени демо: начало, стыки сегментов (сейвлоуд или смена карты,
// если сегмент начался на другой карте) и смены карт внутри сегментов (svc_serverinfo).
// Не больше MAX_MARKERS: таймлайн и ответ demo-inspect не раздуваются от подделанной демки.
function timelineOf(picked, scans, range) {
  const markers = [];
  let lastMap = null;
  picked.forEach((item, k) => {
    const scan = scans[k] || null;
    const changes = (scan && scan.mapChanges) || [];
    const at = round3(k === 0 ? Math.max(item.start, range ? range.from : 0) : item.start);
    // Клип с середины сегмента — карта, которая шла к его началу.
    const before = k === 0 ? changes.filter((change) => change.t > 0 && item.start + change.t <= at).pop() : null;
    const map = before ? before.map : item.segment.map || "";
    if (k === 0) markers.push({ t: at, type: "map", label: map });
    else {
      const changed = map && lastMap && map !== lastMap;
      markers.push({ t: at, type: changed ? "map" : "saveload", label: changed ? map : String(item.index + 1) });
    }
    lastMap = map || lastMap;
    for (const change of changes) {
      if (change.map === lastMap || item.start + change.t <= at) continue;
      if (change.t > 0) markers.push({ t: round3(item.start + change.t), type: "map", label: change.map });
      lastMap = change.map;
    }
  });
  return markers.slice(0, MAX_MARKERS);
}

// ---------- KeyHUD ----------

// Клавиши: подпись, бит usercmd, место в сетке (единица — клавиша) и ширина.
const KEYS = [
  { label: "W", bit: BUTTONS.forward, x: 1, y: 0, w: 1 },
  { label: "A", bit: BUTTONS.moveleft, x: 0, y: 1, w: 1 },
  { label: "S", bit: BUTTONS.back, x: 1, y: 1, w: 1 },
  { label: "D", bit: BUTTONS.moveright, x: 2, y: 1, w: 1 },
  { label: "DUCK", bit: BUTTONS.duck, x: 0, y: 2, w: 1.5 },
  { label: "JUMP", bit: BUTTONS.jump, x: 1.5, y: 2, w: 1.5 },
  { label: "USE", bit: BUTTONS.use, x: 0, y: 3, w: 1 },
  { label: "ATK", bit: BUTTONS.attack, x: 1, y: 3, w: 1 },
  { label: "ATK2", bit: BUTTONS.attack2, x: 2, y: 3, w: 1 },
];

function assTime(seconds) {
  const cs = Math.max(0, Math.round(seconds * 100));
  const h = Math.floor(cs / 360000);
  const m = Math.floor((cs % 360000) / 6000);
  const s = Math.floor((cs % 6000) / 100);
  return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}.${String(cs % 100).padStart(2, "0")}`;
}

// Нажатия -> время материала: { t: время демо, buttons } каждого сегмента
// переводятся через offset и вырезанное (как метки таймлайна в jobs.js).
function keyEvents(picked, scans, timing) {
  const events = [];
  picked.forEach((item, k) => {
    const scan = scans[k];
    for (const press of (scan && scan.buttons) || []) {
      const t = Math.min(item.start + press.t, item.end);
      events.push({ t: planner.materialTime(t, timing), buttons: press.buttons });
    }
    // На стыке сегментов кнопки сбрасываются: следующий сегмент начнёт со своих.
    events.push({ t: planner.materialTime(item.end, timing), buttons: 0 });
  });
  return events.sort((a, b) => a.t - b.t);
}

// Субтитры ASS с векторными клавишами (libass в FFmpeg): серые клавиши всё
// время, нажатая — подсвечена на время нажатия. Размер — от высоты кадра,
// место — справа над патронами HUD (снизу по центру — спидометр BXT игрока).
// end — длительность материала.
function keyhudAss(events, { width, height, end }) {
  const scale = height / 1080;
  const unit = Math.round(44 * scale);
  const gap = Math.max(2, Math.round(6 * scale));
  const font = Math.round(19 * scale);
  const left = Math.round(width - height * 0.04 - (3 * unit + 2 * gap));
  const top = Math.round(height * 0.8 - (4 * unit + 3 * gap));
  const box = (key) => {
    const x = left + Math.round(key.x * (unit + gap));
    const y = top + key.y * (unit + gap);
    const w = Math.round(key.w * unit + (key.w - 1) * gap);
    return { x, y, w, h: unit };
  };
  const rect = (b, color, alpha) =>
    `{\\an7\\pos(${b.x},${b.y})\\bord0\\shad0\\1c&H${color}&\\1a&H${alpha}&\\p1}m 0 0 l ${b.w} 0 ${b.w} ${b.h} 0 ${b.h}{\\p0}`;
  const lines = [
    "[Script Info]",
    "; KeyHUD HLSR: нажатия из usercmd демки",
    "ScriptType: v4.00+",
    `PlayResX: ${width}`,
    `PlayResY: ${height}`,
    "ScaledBorderAndShadow: yes",
    "WrapStyle: 2",
    "",
    "[V4+ Styles]",
    "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding",
    `Style: K,Arial,${font},&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,1,0,0,0,100,100,0,0,1,${Math.max(1, Math.round(scale))},0,5,0,0,0,1`,
    "",
    "[Events]",
    "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text",
  ];
  const whole = `${assTime(0)},${assTime(end)}`;
  for (const key of KEYS) {
    const b = box(key);
    lines.push(`Dialogue: 0,${whole},K,,0,0,0,,${rect(b, "000000", "70")}`);
    lines.push(`Dialogue: 2,${whole},K,,0,0,0,,{\\an5\\pos(${b.x + Math.round(b.w / 2)},${b.y + Math.round(b.h / 2)})}${key.label}`);
  }
  // Нажатия: интервалы, пока бит клавиши стоит (HL1 HUD — оранжевый).
  for (const key of KEYS) {
    const b = box(key);
    let since = null;
    const close = (t) => {
      const a = Math.round(since * 100);
      const z = Math.round(Math.min(t, end) * 100);
      if (z > a) lines.push(`Dialogue: 1,${assTime(a / 100)},${assTime(z / 100)},K,,0,0,0,,${rect(b, "00A0FF", "30")}`);
      since = null;
    };
    for (const event of events) {
      const down = (event.buttons & key.bit) !== 0;
      if (down && since === null) since = event.t;
      else if (!down && since !== null) close(event.t);
    }
    if (since !== null) close(end);
  }
  return lines.join("\n") + "\n";
}

module.exports = {
  RENDER_DIR,
  NET_PROTOCOL,
  CAPTURE_VOLUME,
  MAX_MARKERS,
  KEYS,
  planSegments,
  jobConfig,
  gameArguments,
  resetArguments,
  parseLogLine,
  progressSeconds,
  lossCuts,
  timelineOf,
  assTime,
  keyEvents,
  keyhudAss,
};
