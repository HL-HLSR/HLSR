"use strict";

// Чистая логика страницы «Демо» (без Vue и IPC): время, сочетания
// разрешение × FPS, тексты ошибок по кодам главного процесса.
// Самотест: scripts/demo-tool-ui-selftest.js.

const RATES = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2];
const CLIP_SPEEDS = [0.25, 0.5, 1, 1.5, 2, 4];
const RESOLUTIONS = [
  { height: 720, label: "720p" },
  { height: 1080, label: "1080p" },
  { height: 1440, label: "1440p" },
  { height: 2160, label: "4K" },
];

const GROUP_ORDER = ["hl1-won", "ghosting-renewed", "hl2-4260", "hl2-2707", "hl2-5288"];
const GROUP_NAMES = {
  "hl1-won": "Half-Life",
  "ghosting-renewed": "Ghosting Mod: Renewed",
  "hl2-4260": "Half-Life 2 (4260)",
  "hl2-2707": "Half-Life 2 (2707)",
  "hl2-5288": "Half-Life 2 (5288)",
};
const GAME_NAMES = { hl1: "Half-Life", ghosting: "Ghosting", hl2: "Half-Life 2" };
const ENCODER_NAMES = {
  h264_nvenc: "NVIDIA NVENC",
  h264_amf: "AMD AMF",
  h264_qsv: "Intel Quick Sync",
  libx264: "x264",
};

// Секунды → «1:05.3» (или «1:02:05.3»); digits — знаков после точки.
function formatClock(seconds, digits) {
  const places = digits === undefined ? 1 : digits;
  const value = Math.max(0, Number(seconds) || 0);
  const scale = Math.pow(10, places);
  const total = Math.round(value * scale) / scale;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total - hours * 3600 - minutes * 60;
  let secs = rest.toFixed(places);
  if (rest < 10) secs = "0" + secs;
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${secs}` : `${minutes}:${secs}`;
}

// «1:05.3», «65,3», «1:02:05» → секунды; непонятное → null.
function parseClock(text) {
  const parts = String(text === undefined || text === null ? "" : text).trim().replace(",", ".").split(":");
  if (!parts[0] && parts.length === 1) return null;
  if (parts.length > 3 || !parts.every((part) => /^\d+(\.\d+)?$/.test(part))) return null;
  const numbers = parts.map(Number);
  if (numbers.slice(1).some((value) => value >= 60)) return null;
  if (numbers.slice(0, -1).some((value) => !Number.isInteger(value))) return null;
  return numbers.reduce((sum, value) => sum * 60 + value, 0);
}

// Время видео → время демо. video — timeline.video ({ range, speed, duration });
// markers — метки видео ({ t, sourceTime }): ближайшая метка слева учитывает
// вырезанные загрузки, дальше время идёт со скоростью клипа.
function demoTimeAt(videoTime, video, markers) {
  const range = (video && video.range) || { from: 0, to: Infinity };
  const speed = (video && video.speed) || 1;
  let anchor = { t: 0, sourceTime: range.from };
  for (const marker of markers || [])
    if (Number.isFinite(marker.sourceTime) && marker.t <= videoTime && marker.t >= anchor.t) anchor = marker;
  const value = anchor.sourceTime + (videoTime - anchor.t) * speed;
  return Math.min(range.to, Math.max(range.from, value));
}

// Обратное: время демо → время видео (для подсветки интервала на полосе).
function videoTimeAt(demoTime, video, markers) {
  const range = (video && video.range) || { from: 0, to: Infinity };
  const speed = (video && video.speed) || 1;
  let anchor = { t: 0, sourceTime: range.from };
  for (const marker of markers || [])
    if (Number.isFinite(marker.sourceTime) && marker.sourceTime <= demoTime && marker.sourceTime >= anchor.sourceTime) anchor = marker;
  return Math.max(0, anchor.t + (demoTime - anchor.sourceTime) / speed);
}

// Допустимые сочетания для режима кодировщика из ответа demo-options.
// FFmpeg не готов (modes: null) — всё, что умеет адаптер: выбрать можно,
// проверит главный процесс, когда FFmpeg появится.
function combosFor(options, mode) {
  if (!options) return [];
  if (options.modes) {
    const entry = options.modes[mode];
    return entry && Array.isArray(entry.combos) ? entry.combos : [];
  }
  const caps = options.capabilities || {};
  const combos = [];
  for (const resolution of options.resolutions || [])
    for (const fps of caps.fps || []) combos.push({ height: resolution.height, width: resolution.width, fps });
  return combos;
}

function heightsOf(combos) {
  return [...new Set((combos || []).map((combo) => combo.height))].sort((a, b) => a - b);
}

function fpsFor(combos, height) {
  return [...new Set((combos || []).filter((combo) => combo.height === height).map((combo) => combo.fps))].sort((a, b) => a - b);
}

// Ближайшее допустимое к желаемому: разрешение не выше выбранного (иначе
// самое низкое), FPS не выше выбранного (иначе самый низкий).
function pickCombo(combos, height, fps) {
  const heights = heightsOf(combos);
  if (!heights.length) return null;
  const lowerHeights = heights.filter((value) => value <= height);
  const h = heights.includes(height) ? height : lowerHeights.length ? lowerHeights[lowerHeights.length - 1] : heights[0];
  const rates = fpsFor(combos, h);
  const lowerRates = rates.filter((value) => value <= fps);
  const f = rates.includes(fps) ? fps : lowerRates.length ? lowerRates[lowerRates.length - 1] : rates[0];
  return { height: h, fps: f };
}

// Оценка времени задания -> «~45 с», «~21 мин», «~1 ч 05 мин» ({ key, args } для $t).
function durationText(seconds) {
  const value = Math.max(0, Number(seconds) || 0);
  if (value < 60) return { key: "#L_DEMO_EST_SEC", args: [String(Math.max(5, Math.ceil(value / 5) * 5))] };
  const minutes = Math.ceil(value / 60);
  if (minutes < 60) return { key: "#L_DEMO_EST_MIN", args: [String(minutes)] };
  return { key: "#L_DEMO_EST_HOUR", args: [String(Math.floor(minutes / 60)), String(minutes % 60).padStart(2, "0")] };
}

function resolutionLabel(height) {
  const found = RESOLUTIONS.find((item) => item.height === height);
  return found ? found.label : `${height}p`;
}

function encoderName(id) {
  return ENCODER_NAMES[id] || id || "";
}

function gameName(game) {
  return GAME_NAMES[game] || "";
}

// Номер главы HL2 из метки (source-demo.js CHAPTERS) → ключ её названия.
const CHAPTER_KEYS = {
  1: "#L_DEMO_CHAPTER_1",
  2: "#L_DEMO_CHAPTER_2",
  3: "#L_DEMO_CHAPTER_3",
  4: "#L_DEMO_CHAPTER_4",
  5: "#L_DEMO_CHAPTER_5",
  6: "#L_DEMO_CHAPTER_6",
  7: "#L_DEMO_CHAPTER_7",
  8: "#L_DEMO_CHAPTER_8",
  9: "#L_DEMO_CHAPTER_9",
  "9a": "#L_DEMO_CHAPTER_9A",
  10: "#L_DEMO_CHAPTER_10",
  11: "#L_DEMO_CHAPTER_11",
  12: "#L_DEMO_CHAPTER_12",
  13: "#L_DEMO_CHAPTER_13",
};

function chapterKey(id) {
  return Object.prototype.hasOwnProperty.call(CHAPTER_KEYS, String(id)) ? CHAPTER_KEYS[String(id)] : null;
}

// Что получает плеер: видео (предпросмотр или экспорт) только выбранной
// записи. Иначе шкала — время её демки с метками источника: чужой таймлайн
// попал бы в «Начало/Конец здесь» и в переходы по меткам.
function playerInput(media, detail) {
  const own = !!(media && detail && media.itemId === detail.item.id);
  if (own) return { own, src: media.result.mediaUrl, video: media.result.video, markers: media.result.markers || [] };
  return { own, src: null, video: null, markers: (detail && detail.markers) || [] };
}

// Итог экспорта ({ file, fileName }) — это видео папки вывода: после удаления — по точному
// пути из ответа (file), до него — по имени и папке. Регистр путей Windows не важен.
function isRenderedFile(result, { name, outputDir, file }) {
  if (!result || !result.file) return false;
  if (file) return result.file.toLowerCase() === String(file).toLowerCase();
  const dir = result.file.replace(/[\\/][^\\/]*$/, "");
  return result.fileName === name && dir.toLowerCase() === String(outputDir || "").replace(/[\\/]+$/, "").toLowerCase();
}

// Отметка интервала: время демки в пределах рана. Конец раньше начала (или
// начало позже конца) — начало и конец меняются местами, swapped говорит
// интерфейсу сказать об этом. Весь ран и пустой отрезок — range: null.
function markRange(range, which, demoTime, duration) {
  const length = Math.max(0, Number(duration) || 0);
  const next = Object.assign({ from: 0, to: length }, range, { [which]: Math.min(length, Math.max(0, Number(demoTime) || 0)) });
  const swapped = next.to < next.from;
  if (swapped) [next.from, next.to] = [next.to, next.from];
  if (next.to <= next.from || (next.from <= 0 && next.to >= length)) return { range: null, swapped: false };
  return { range: next, swapped };
}

// Выбор по умолчанию: первая поддерживаемая запись, иначе первая.
function firstPick(groups) {
  const all = [].concat(...(groups || []).map((group) => group.items));
  const pick = all.find((item) => item.supported) || all[0];
  return pick ? pick.id : null;
}

// Группы списка: игры в постоянном порядке, затем добавленные вручную.
function groupItems(items) {
  const groups = new Map();
  for (const item of items || []) {
    const key = item.source === "manual" ? "manual" : item.gameId || "other";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(item);
  }
  const rank = (key) => (key === "manual" ? 1000 : GROUP_ORDER.includes(key) ? GROUP_ORDER.indexOf(key) : 500);
  return [...groups.entries()]
    .sort((a, b) => rank(a[0]) - rank(b[0]) || a[0].localeCompare(b[0]))
    .map(([id, list]) => ({
      id,
      name: GROUP_NAMES[id] || null,
      // Поддерживаемые выше, внутри — новые сверху.
      items: list.slice().sort((a, b) => (b.supported ? 1 : 0) - (a.supported ? 1 : 0) || (b.mtimeMs || 0) - (a.mtimeMs || 0)),
    }));
}

const DEMO_EXTENSIONS = [".dem", ".ghplaylist"];

function hasDemoExtension(name) {
  const lower = String(name || "").toLowerCase();
  return DEMO_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

// Код главного процесса → ключ текста. Неизвестный код — общий текст с кодом.
const ERROR_KEYS = {
  CANCELLED: "#L_DEMO_ERR_CANCELLED",
  RENDER_NOT_READY: "#L_DEMO_ERR_RENDER_NOT_READY",
  RENDER_FAILED: "#L_DEMO_ERR_RENDER_FAILED",
  DEMO_UNSUPPORTED: "#L_DEMO_ERR_UNSUPPORTED",
  DEMO_PROTOCOL_UNSUPPORTED: "#L_DEMO_ERR_PROTOCOL",
  DEMO_TRUNCATED: "#L_DEMO_ERR_TRUNCATED",
  DEMO_HEADER_INVALID: "#L_DEMO_ERR_BROKEN",
  DEMO_DIRECTORY_INVALID: "#L_DEMO_ERR_BROKEN",
  DEMO_NOT_A_DEMO: "#L_DEMO_ERR_NOT_DEMO",
  DEMO_NOT_FOUND: "#L_DEMO_ERR_NOT_FOUND",
  DEMO_READ_FAILED: "#L_DEMO_ERR_READ",
  PLAYLIST_INVALID: "#L_DEMO_ERR_PLAYLIST",
  PLAYLIST_PATH_ESCAPE: "#L_DEMO_ERR_PLAYLIST_PATH",
  SEGMENT_MISSING: "#L_DEMO_ERR_SEGMENT_MISSING",
  SEGMENTS_MIXED: "#L_DEMO_ERR_MIXED",
  DURATION_UNKNOWN: "#L_DEMO_ERR_DURATION",
  FILE_TYPE_UNSUPPORTED: "#L_DEMO_ERR_FILE_TYPE",
  PATH_INVALID: "#L_DEMO_ERR_PATH",
  LIST_FULL: "#L_DEMO_ERR_LIST_FULL",
  FFMPEG_MISSING: "#L_DEMO_ERR_FFMPEG_MISSING",
  FFMPEG_NOT_IN_PATH: "#L_DEMO_ERR_FFMPEG_PATH",
  FFMPEG_BROKEN: "#L_DEMO_ERR_FFMPEG_BROKEN",
  FFMPEG_VERSION_MISMATCH: "#L_DEMO_ERR_FFMPEG_VERSION",
  FFMPEG_FAILED: "#L_DEMO_ERR_FFMPEG_FAILED",
  FFMPEG_START_FAILED: "#L_DEMO_ERR_FFMPEG_START",
  FFMPEG_TIMEOUT: "#L_DEMO_ERR_FFMPEG_TIMEOUT",
  FFMPEG_INSTALL_UNSUPPORTED: "#L_DEMO_ERR_FFMPEG_PATH",
  VERIFY_FAILED: "#L_DEMO_ERR_VERIFY",
  OUTPUT_WRITE_FAILED: "#L_DEMO_ERR_WRITE",
  OUTPUT_DIR_UNAVAILABLE: "#L_DEMO_ERR_OUTPUT_DIR",
  NO_SPACE: "#L_DEMO_ERR_NO_SPACE",
  JOB_BUSY: "#L_DEMO_ERR_BUSY",
  COMBO_UNSUPPORTED: "#L_DEMO_ERR_COMBO",
  ENCODER_GPU_UNAVAILABLE: "#L_DEMO_ERR_NO_GPU",
  ENCODER_UNAVAILABLE: "#L_DEMO_ERR_NO_ENCODER",
  RANGE_INVALID: "#L_DEMO_ERR_RANGE",
  RESULT_NOT_FOUND: "#L_DEMO_ERR_RESULT_GONE",
  TIMELINE_INVALID: "#L_DEMO_ERR_TIMELINE",
  NETWORK_ERROR: "#L_DEMO_ERR_NETWORK",
  NETWORK_TIMEOUT: "#L_DEMO_ERR_NETWORK",
  SIZE_MISMATCH: "#L_DEMO_ERR_DOWNLOAD",
  CHECKSUM_MISMATCH: "#L_DEMO_ERR_DOWNLOAD",
  EXTRACT_FAILED: "#L_DEMO_ERR_DOWNLOAD",
  ANTIVIRUS_BLOCKED: "#L_DEMO_ERR_ANTIVIRUS",
  PATH_NOT_WRITABLE: "#L_DEMO_ERR_TOOLS_DIR",
  ALREADY_RUNNING: "#L_DEMO_ERR_INSTALLING",
  FFMPEG_INSTALLING: "#L_DEMO_ERR_FFMPEG_INSTALLING",
  JOBS_DIR_UNAVAILABLE: "#L_DEMO_ERR_JOBS_DIR",
  JOBS_DIR_UNSAFE: "#L_DEMO_ERR_JOBS_DIR_UNSAFE",
  STEAM_NOT_RUNNING: "#L_DEMO_ERR_STEAM_NOT_RUNNING",
  GAME_RUNNING: "#L_DEMO_ERR_GAME_RUNNING",
  // Тот же текст, что у запуска игры во время записи (LauncherShell: #L_D_ERR_<код>).
  RENDER_ACTIVE: "#L_D_ERR_RENDER_ACTIVE",
  // Рендер Ghosting (adapter ghosting.js): предусловия и сбои игры.
  STEAM_NOT_INSTALLED: "#L_DEMO_ERR_STEAM_NOT_INSTALLED",
  STEAM_ELEVATED: "#L_DEMO_ERR_STEAM_ELEVATED",
  SEGMENT_CHANGED: "#L_DEMO_ERR_SEGMENT_CHANGED",
  MAP_MISSING: "#L_DEMO_ERR_MAP_MISSING",
  DEMO_OPEN_FAILED: "#L_DEMO_ERR_DEMO_OPEN",
  GAME_ERROR: "#L_DEMO_ERR_GAME_ERROR",
  GAME_EXITED: "#L_DEMO_ERR_GAME_EXITED",
  RENDER_TIMEOUT: "#L_DEMO_ERR_RENDER_TIMEOUT",
  RENDER_HOST_MISSING: "#L_DEMO_ERR_HOST_MISSING",
  RENDER_HOST_FAILED: "#L_DEMO_ERR_HOST_FAILED",
  REGISTRY_FAILED: "#L_DEMO_ERR_REGISTRY",
  AUDIO_MISSING: "#L_DEMO_ERR_AUDIO_MISSING",
  AUDIO_INVALID: "#L_DEMO_ERR_AUDIO_INVALID",
  GAME_NOT_INSTALLED: "#L_DEMO_ERR_GAME_NOT_INSTALLED",
  DEMO_EMPTY: "#L_DEMO_ERR_DEMO_EMPTY",
  // Остальные коды каналов demo-* и установки FFmpeg (ToolService, ArchiveService).
  DIALOG_UNAVAILABLE: "#L_DEMO_ERR_DIALOG",
  ORDER_INVALID: "#L_DEMO_ERR_ORDER",
  SETTINGS_INVALID: "#L_DEMO_ERR_SETTINGS",
  TOOLS_UNAVAILABLE: "#L_DEMO_ERR_TOOLS",
  UNTRUSTED: "#L_DEMO_ERR_UNTRUSTED",
  REMOTE_DISABLED: "#L_DEMO_ERR_REMOTE_DISABLED",
  MANIFEST_INVALID: "#L_DEMO_ERR_MANIFEST",
  ARCHIVE_LIMIT_EXCEEDED: "#L_DEMO_ERR_DOWNLOAD",
  ARCHIVE_LIST_FAILED: "#L_DEMO_ERR_DOWNLOAD",
  ARCHIVE_UNSAFE_ENTRY: "#L_DEMO_ERR_DOWNLOAD",
  NOT_INSTALLED: "#L_DEMO_ERR_FFMPEG_MISSING",
  // Раунд исправлений рендера Ghosting.
  GAME_BUSY: "#L_DEMO_ERR_GAME_BUSY",
  RESOLUTION_LIMITED: "#L_DEMO_ERR_RESOLUTION_LIMITED",
  // Рендер HL1 через bxt-rs (adapter goldsrc.js) и его установка.
  BXTRS_MISSING: "#L_DEMO_ERR_BXTRS_MISSING",
  BXTRS_BROKEN: "#L_DEMO_ERR_BXTRS_BROKEN",
  BXTRS_FAILED: "#L_DEMO_ERR_BXTRS_FAILED",
  BXTRS_INSTALL_UNSUPPORTED: "#L_DEMO_ERR_RENDER_NOT_READY",
  GOLDSRC_RUNNING: "#L_DEMO_ERR_GOLDSRC_RUNNING",
  FFMPEG_IN_GAME_DIR: "#L_DEMO_ERR_FFMPEG_IN_GAME_DIR",
  SEGMENT_REPLACED: "#L_DEMO_ERR_SEGMENT_REPLACED",
  DEMO_TOO_LARGE: "#L_DEMO_ERR_READ",
  // Режим мода Ghosting -ghrender (ghrender.js): протокол журнала и коды FAIL.
  GHRENDER_UNSUPPORTED: "#L_DEMO_ERR_GHRENDER_UNSUPPORTED",
  BAD_OPTION: "#L_DEMO_ERR_BAD_OPTION",
  SEGMENT_MALFORMED: "#L_DEMO_ERR_SEGMENT_MALFORMED",
  SEGMENT_UNSAFE: "#L_DEMO_ERR_SEGMENT_UNSAFE",
  // Сбой без подробностей (publicError: fallback каналов и заданий).
  DEMO_FAILED: "#L_DEMO_ERR_FAILED",
  JOB_FAILED: "#L_DEMO_ERR_FAILED",
  KEYHUD_NOT_AVAILABLE: "#L_DEMO_EX_KEYHUD_NA",
  // Вкладка «Отрендеренные»: видео не ушло в Корзину.
  TRASH_FAILED: "#L_DEMO_ERR_TRASH",
};

// Предупреждения плана и итога; неизвестные не показываются.
const WARNING_KEYS = {
  DISCORD_LOW_QUALITY: "#L_DEMO_WARN_DISCORD_LOW_QUALITY",
  CPU_SLOW: "#L_DEMO_WARN_CPU_SLOW",
  KEYHUD_UNAVAILABLE: "#L_DEMO_EX_KEYHUD_NA",
  AUDIO_UNAVAILABLE: "#L_DEMO_EX_AUDIO_NA",
  KEYHUD_PARTIAL: "#L_DEMO_WARN_KEYHUD_PARTIAL",
};

function warningKeys(warnings) {
  return [...new Set((warnings || []).map((code) => WARNING_KEYS[code]).filter(Boolean))];
}

// { key, args } для $t. DISCORD_TOO_LONG — с советом, что укоротить.
function errorMessage(error) {
  const code = (error && error.code) || "JOB_FAILED";
  if (code === "DISCORD_TOO_LONG") {
    const seconds = String(Math.max(0, Math.floor(Number(error.recommendedMaxSeconds) || 0)));
    return error.suggestedHeight
      ? { key: "#L_DEMO_ERR_DISCORD_LONG_RES", args: [seconds, resolutionLabel(error.suggestedHeight)] }
      : { key: "#L_DEMO_ERR_DISCORD_LONG", args: [seconds] };
  }
  const value = (field) => String(error[field] === undefined || error[field] === null ? "?" : error[field]);
  if (code === "FFMPEG_FAILED") return { key: ERROR_KEYS[code], args: [value("exitCode")] };
  // У HL1 кода выхода игры нет (запускает Injector) — текст без него.
  if (code === "GAME_EXITED")
    return error.exitCode === undefined || error.exitCode === null ? { key: ERROR_KEYS[code] + "_ANY", args: [] } : { key: ERROR_KEYS[code], args: [value("exitCode")] };
  // Сегмент: номер (с 1) и имя файла, что из них есть.
  if (code === "SEGMENT_MISSING" || code === "SEGMENT_CHANGED" || code === "SEGMENT_REPLACED") {
    const number = error.segment ? String(error.segment) : "";
    const name = error.name ? String(error.name) : "";
    if (number && name) return { key: ERROR_KEYS[code] + "_NAMED", args: [number, name] };
    if (number || name) return { key: ERROR_KEYS[code], args: [number || name] };
    return { key: ERROR_KEYS[code] + "_ANY", args: [] };
  }
  // -ghrender: только номер записи плейлиста.
  if (code === "SEGMENT_MALFORMED" || code === "SEGMENT_UNSAFE")
    return error.segment ? { key: ERROR_KEYS[code], args: [String(error.segment)] } : { key: ERROR_KEYS[code] + "_ANY", args: [] };
  if (code === "GAME_NOT_INSTALLED" && error.gameName) return { key: "#L_DEMO_ERR_GAME_NOT_INSTALLED_NAMED", args: [String(error.gameName)] };
  if (code === "SETTINGS_INVALID") return { key: ERROR_KEYS[code], args: [value("field")] };
  if (code === "RESOLUTION_LIMITED") return { key: ERROR_KEYS[code], args: [value("width"), value("height")] };
  // HL1 (bxt-rs) счёта кадров не даёт — текст без чисел.
  if (code === "RENDER_TIMEOUT")
    return error.framesTotal ? { key: ERROR_KEYS[code], args: [value("framesDone"), value("framesTotal")] } : { key: ERROR_KEYS[code] + "_ANY", args: [] };
  if (ERROR_KEYS[code]) return { key: ERROR_KEYS[code], args: [] };
  return { key: "#L_DEMO_ERR_GENERIC", args: [code] };
}

module.exports = {
  RATES,
  CLIP_SPEEDS,
  RESOLUTIONS,
  formatClock,
  parseClock,
  demoTimeAt,
  videoTimeAt,
  combosFor,
  heightsOf,
  fpsFor,
  pickCombo,
  resolutionLabel,
  durationText,
  encoderName,
  gameName,
  chapterKey,
  CHAPTER_KEYS,
  groupItems,
  groupName: (id) => GROUP_NAMES[id] || null,
  playerInput,
  isRenderedFile,
  markRange,
  firstPick,
  hasDemoExtension,
  errorMessage,
  warningKeys,
  ERROR_KEYS,
  WARNING_KEYS,
};
