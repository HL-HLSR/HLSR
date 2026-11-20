"use strict";

// Оверлей LiveSplit в окне игры: настройки, их проверка и маски фона.
// Общий для main (overlay.ini для прокси оверлея) и окна персонализации
// (предпросмотр). Маски повторяют фильтры OBS «Цветовой ключ» и «Яркостный
// ключ» и код оверлея (scripts/livesplit-overlay/common.h, key_alpha): предпросмотр
// должен вырезать те же пиксели, что игра.

const KEYS = ["none", "color", "luma"];
const KEY_CODES = { none: 0, color: 1, luma: 2 };

// x и y — где картинка стоит в свободном месте кадра: 0 — у левого
// (верхнего) края, 100 — у правого (нижнего). height — доля высоты кадра.
const LIMITS = {
  x: [0, 100],
  y: [0, 100],
  height: [5, 100],
  opacity: [0, 100],
  similarity: [1, 1000],
  smoothness: [1, 1000],
  lumaMin: [0, 1],
  lumaMinSmooth: [0, 1],
  lumaMax: [0, 1],
  lumaMaxSmooth: [0, 1],
  crop: [0, 2048],
};

// Углы и края кадра для быстрых кнопок расположения.
const ANCHORS = [
  { id: "top-left", x: 0, y: 0 },
  { id: "top", x: 50, y: 0 },
  { id: "top-right", x: 100, y: 0 },
  { id: "left", x: 0, y: 50 },
  { id: "right", x: 100, y: 50 },
  { id: "bottom-left", x: 0, y: 100 },
  { id: "bottom", x: 50, y: 100 },
  { id: "bottom-right", x: 100, y: 100 },
];

function defaultOverlay() {
  return {
    enabled: false,
    x: 100,
    y: 50,
    height: 45,
    opacity: 100,
    key: "none",
    // Как у OBS: зелёный ключ, сходство 400, сглаживание 80.
    keyColor: { r: 0, g: 255, b: 0 },
    similarity: 400,
    smoothness: 80,
    lumaMin: 0.1,
    lumaMinSmooth: 0.05,
    lumaMax: 1,
    lumaMaxSmooth: 0,
    crop: { top: 0, right: 0, bottom: 0, left: 0 },
  };
}

function number(value, [min, max], fallback, integer) {
  const parsed = typeof value === "number" ? value : typeof value === "string" && value.trim() ? Number(value) : NaN;
  if (!Number.isFinite(parsed)) return fallback;
  const clamped = Math.min(max, Math.max(min, parsed));
  return integer ? Math.round(clamped) : Math.round(clamped * 1000) / 1000;
}

function colorOf(value, fallback) {
  const source = value && typeof value === "object" ? value : {};
  const channel = (name) => number(source[name], [0, 255], fallback[name], true);
  return { r: channel("r"), g: channel("g"), b: channel("b") };
}

function sanitizeOverlay(input) {
  const source = input && typeof input === "object" && !Array.isArray(input) ? input : {};
  const base = defaultOverlay();
  const crop = source.crop && typeof source.crop === "object" ? source.crop : {};
  return {
    enabled: source.enabled === true,
    x: number(source.x, LIMITS.x, base.x),
    y: number(source.y, LIMITS.y, base.y),
    height: number(source.height, LIMITS.height, base.height),
    opacity: number(source.opacity, LIMITS.opacity, base.opacity, true),
    key: KEYS.includes(source.key) ? source.key : base.key,
    keyColor: colorOf(source.keyColor, base.keyColor),
    similarity: number(source.similarity, LIMITS.similarity, base.similarity, true),
    smoothness: number(source.smoothness, LIMITS.smoothness, base.smoothness, true),
    lumaMin: number(source.lumaMin, LIMITS.lumaMin, base.lumaMin),
    lumaMinSmooth: number(source.lumaMinSmooth, LIMITS.lumaMinSmooth, base.lumaMinSmooth),
    lumaMax: number(source.lumaMax, LIMITS.lumaMax, base.lumaMax),
    lumaMaxSmooth: number(source.lumaMaxSmooth, LIMITS.lumaMaxSmooth, base.lumaMaxSmooth),
    crop: {
      top: number(crop.top, LIMITS.crop, 0, true),
      right: number(crop.right, LIMITS.crop, 0, true),
      bottom: number(crop.bottom, LIMITS.crop, 0, true),
      left: number(crop.left, LIMITS.crop, 0, true),
    },
  };
}

// Свободная точка (x, y в процентах) для картинки с левым верхним углом в
// (left, top): обратное к placement, для перетаскивания в предпросмотре.
function positionOf(left, top, screenWidth, screenHeight, width, height) {
  const axis = (start, free) => (free > 0 ? Math.round(Math.min(100, Math.max(0, start / free * 100)) * 10) / 10 : 50);
  return { x: axis(left, screenWidth - width), y: axis(top, screenHeight - height) };
}

// Сколько настроек отличается от стандартных (точка «изменено» у раздела).
// Выключатель не считается: сброс раздела его не трогает.
function overlayChanges(value) {
  const clean = sanitizeOverlay(value);
  const base = defaultOverlay();
  return Object.keys(base).filter((key) => key !== "enabled" && JSON.stringify(clean[key]) !== JSON.stringify(base[key])).length;
}

const hex = (value) => value.toString(16).padStart(2, "0");

// overlay.ini для прокси оверлея (разбор — config_parse в common.h).
function toIni(value) {
  const clean = sanitizeOverlay(value);
  const color = clean.keyColor;
  return [
    `enabled=${clean.enabled ? 1 : 0}`,
    `x=${clean.x}`,
    `y=${clean.y}`,
    `height=${clean.height}`,
    `opacity=${clean.opacity}`,
    `key=${KEY_CODES[clean.key]}`,
    `key_color=${hex(color.r)}${hex(color.g)}${hex(color.b)}`,
    `similarity=${clean.similarity}`,
    `smoothness=${clean.smoothness}`,
    `luma_min=${clean.lumaMin}`,
    `luma_min_smooth=${clean.lumaMinSmooth}`,
    `luma_max=${clean.lumaMax}`,
    `luma_max_smooth=${clean.lumaMaxSmooth}`,
    `crop_top=${clean.crop.top}`,
    `crop_right=${clean.crop.right}`,
    `crop_bottom=${clean.crop.bottom}`,
    `crop_left=${clean.crop.left}`,
    "",
  ].join("\r\n");
}

function smoothstep(edge0, edge1, x) {
  if (edge1 <= edge0) return x < edge0 ? 0 : 1;
  const t = Math.min(1, Math.max(0, (x - edge0) / (edge1 - edge0)));
  return t * t * (3 - 2 * t);
}

// Непрозрачность пикселя 0..1 после маски; r, g, b — 0..1.
function keyAlpha(value, r, g, b) {
  if (value.key === "color") {
    const color = value.keyColor;
    const dr = r - color.r / 255, dg = g - color.g / 255, db = b - color.b / 255;
    const base = Math.sqrt(dr * dr + dg * dg + db * db) - value.similarity / 1000;
    if (base <= 0) return 0;
    return Math.min(1, base / (value.smoothness / 1000));
  }
  if (value.key === "luma") {
    const luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    const low = smoothstep(value.lumaMin, value.lumaMin + value.lumaMinSmooth, luma);
    const high = value.lumaMaxSmooth <= 0
      ? (luma > value.lumaMax ? 0 : 1)
      : 1 - smoothstep(value.lumaMax - value.lumaMaxSmooth, value.lumaMax, luma);
    return low * high;
  }
  return 1;
}

// Обрезка снимка; если от картинки остаётся меньше 8 пикселей, обрезка по
// этой оси не применяется (так же делает DLL).
function cropRect(value, width, height) {
  const crop = sanitizeOverlay(value).crop;
  let { left, right, top, bottom } = crop;
  if (left + right > width - 8) { left = 0; right = 0; }
  if (top + bottom > height - 8) { top = 0; bottom = 0; }
  return { x: left, y: top, width: width - left - right, height: height - top - bottom };
}

// Где картинка встанет в кадре screenWidth x screenHeight (как place в DLL).
function placement(value, screenWidth, screenHeight, width, height) {
  if (screenWidth < 16 || screenHeight < 16 || width < 1 || height < 1) return null;
  const clean = sanitizeOverlay(value);
  let h = screenHeight * clean.height / 100;
  let w = h * width / height;
  if (w > screenWidth) { w = screenWidth; h = w * height / width; }
  const outWidth = Math.round(w), outHeight = Math.round(h);
  return {
    left: Math.round((screenWidth - outWidth) * clean.x / 100),
    top: Math.round((screenHeight - outHeight) * clean.y / 100),
    width: outWidth,
    height: outHeight,
  };
}

// Цвет (B, G, R, X), которым DLL заливает холст перед PrintWindow; что им
// осталось, окно LiveSplit не нарисовало (SENTINEL в common.h).
const SENTINEL = [0x5a, 0xff, 0x00, 0x5a];

function isSentinel(bgrx, at) {
  return bgrx[at] === SENTINEL[0] && bgrx[at + 1] === SENTINEL[1] && bgrx[at + 2] === SENTINEL[2] && bgrx[at + 3] === SENTINEL[3];
}

// Цвет фона LiveSplit для цветового ключа: левый нижний угол снимка, где у
// стандартного макета нет текста. Незакрашенные полосы пропускаются.
function backgroundColor(bgrx, width, height) {
  for (let step = 2; step < Math.min(width, height); step += 2) {
    const at = ((height - step) * width + step) * 4;
    if (!isSentinel(bgrx, at)) return { r: bgrx[at + 2], g: bgrx[at + 1], b: bgrx[at] };
  }
  return null;
}

// Снимок окна LiveSplit (BGRX, сверху вниз) → RGBA для canvas, с обрезкой
// и маской. Альфа не предумножена: canvas ждёт обычную. order "rgba" — для
// примерного LiveSplit, нарисованного в canvas.
function keyImage(value, bgrx, width, height, order = "bgrx") {
  const red = order === "rgba" ? 0 : 2, blue = order === "rgba" ? 2 : 0;
  const clean = sanitizeOverlay(value);
  const rect = cropRect(clean, width, height);
  const out = new Uint8ClampedArray(rect.width * rect.height * 4);
  for (let y = 0; y < rect.height; y++) {
    let source = ((y + rect.y) * width + rect.x) * 4;
    let target = y * rect.width * 4;
    for (let x = 0; x < rect.width; x++, source += 4, target += 4) {
      const r = bgrx[source + red], g = bgrx[source + 1], b = bgrx[source + blue];
      // Незакрашенное место прозрачно при любой маске, как в DLL.
      if (order === "bgrx" && isSentinel(bgrx, source)) continue;
      out[target] = r;
      out[target + 1] = g;
      out[target + 2] = b;
      out[target + 3] = clean.key === "none" ? 255 : Math.round(keyAlpha(clean, r / 255, g / 255, b / 255) * 255);
    }
  }
  return { width: rect.width, height: rect.height, data: out };
}

module.exports = {
  KEYS,
  LIMITS,
  ANCHORS,
  defaultOverlay,
  sanitizeOverlay,
  overlayChanges,
  toIni,
  keyAlpha,
  cropRect,
  placement,
  positionOf,
  keyImage,
  backgroundColor,
};
