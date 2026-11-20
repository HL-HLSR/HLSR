"use strict";

// Значки годовщины в HLSR (1–10 лет): из картинок владельца (badge_01..10.png,
// 1254×1254 RGBA, фон уже прозрачный, свечение нарисовано в картинке) —
// src/renderer/assets/launcher/badges/anniversary-N.png 96×96, как соседние
// значки. Все десять режутся одной рамкой (общая граница непрозрачного с
// небольшим полем): квадрат значка одного размера на всех уровнях. Уменьшение —
// усреднение по площади в premultiplied-цветах, чтобы по краю не было тёмной
// каймы. В конце печатает цвета свечения для src/shared/anniversary-badges.js.
//
//   node scripts/prepare-anniversary-badges.js <папка с badge_01..10.png>
//
// Только Node без зависимостей: PNG читается и пишется через zlib.

const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const SIZE = 96;
const COUNT = 10;
const MARGIN = 0.03; // поле вокруг содержимого, доля стороны рамки
const NOISE = 8; // альфа до 8/255 — шум генератора, а не свечение
const OUT = path.join(__dirname, "../src/renderer/assets/launcher/badges");

const SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

// PNG 8 бит RGB или RGBA без чересстрочности -> { width, height, data: RGBA }.
function decodePng(file) {
  if (!file.subarray(0, 8).equals(SIGNATURE)) throw new Error("не PNG");
  let width = 0;
  let height = 0;
  let type = -1;
  const idat = [];
  for (let at = 8; at < file.length; ) {
    const length = file.readUInt32BE(at);
    const name = file.toString("latin1", at + 4, at + 8);
    const body = file.subarray(at + 8, at + 8 + length);
    if (name === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      type = body[9];
      if (body[8] !== 8 || (type !== 2 && type !== 6) || body[12] !== 0) throw new Error("нужен PNG 8 бит RGB/RGBA без чересстрочности");
    } else if (name === "IDAT") idat.push(body);
    else if (name === "IEND") break;
    at += 12 + length;
  }
  const bpp = type === 6 ? 4 : 3;
  const stride = width * bpp;
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const rows = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const out = rows.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? rows.subarray((y - 1) * stride, y * stride) : Buffer.alloc(stride);
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? out[x - bpp] : 0;
      const b = prev[x];
      const c = x >= bpp ? prev[x - bpp] : 0;
      const predictor = [0, a, b, (a + b) >> 1, paeth(a, b, c)][filter];
      if (predictor === undefined) throw new Error("неизвестный фильтр строки " + filter);
      out[x] = (line[x] + predictor) & 0xff;
    }
  }
  if (bpp === 4) return { width, height, data: rows };
  const data = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    rows.copy(data, i * 4, i * 3, i * 3 + 3);
    data[i * 4 + 3] = 255;
  }
  return { width, height, data };
}

function chunk(name, body) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(body.length, 0);
  head.write(name, 4, "latin1");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])), 0);
  return Buffer.concat([head, body, crc]);
}

// RGBA -> PNG; фильтр строки — с наименьшей суммой модулей (обычная эвристика libpng).
function encodePng({ width, height, data }) {
  const stride = width * 4;
  const lines = [];
  for (let y = 0; y < height; y++) {
    const line = data.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? data.subarray((y - 1) * stride, y * stride) : Buffer.alloc(stride);
    let best = null;
    let bestScore = Infinity;
    for (let filter = 0; filter < 5; filter++) {
      const out = Buffer.alloc(stride + 1);
      out[0] = filter;
      let score = 0;
      for (let x = 0; x < stride; x++) {
        const a = x >= 4 ? line[x - 4] : 0;
        const b = prev[x];
        const c = x >= 4 ? prev[x - 4] : 0;
        const value = (line[x] - [0, a, b, (a + b) >> 1, paeth(a, b, c)][filter]) & 0xff;
        out[x + 1] = value;
        score += value < 128 ? value : 256 - value;
      }
      if (score < bestScore) [best, bestScore] = [out, score];
    }
    lines.push(best);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8);
  return Buffer.concat([SIGNATURE, chunk("IHDR", header), chunk("IDAT", zlib.deflateSync(Buffer.concat(lines), { level: 9 })), chunk("IEND", Buffer.alloc(0))]);
}

// Граница пикселей с альфой выше шума: [x0, y0, x1, y1), x1/y1 не включая.
function contentBox({ width, height, data }) {
  let [x0, y0, x1, y1] = [width, height, 0, 0];
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++)
      if (data[(y * width + x) * 4 + 3] > NOISE) {
        x0 = Math.min(x0, x);
        y0 = Math.min(y0, y);
        x1 = Math.max(x1, x + 1);
        y1 = Math.max(y1, y + 1);
      }
  return [x0, y0, x1, y1];
}

// Квадрат side×side с левым верхним углом (left, top) — в size×size: каждый
// пиксель результата — среднее по своей площади исходника (дробные края — с
// весом), в premultiplied-цветах. Вне исходника — прозрачно.
function resample(image, left, top, side, size) {
  const scale = side / size;
  const out = Buffer.alloc(size * size * 4);
  const spans = (start) => {
    const list = [];
    for (let p = Math.floor(start); p < start + scale; p++) {
      const weight = Math.min(p + 1, start + scale) - Math.max(p, start);
      if (weight > 0) list.push([p, weight]);
    }
    return list;
  };
  for (let oy = 0; oy < size; oy++) {
    const ys = spans(top + oy * scale);
    for (let ox = 0; ox < size; ox++) {
      const xs = spans(left + ox * scale);
      let [r, g, b, a] = [0, 0, 0, 0];
      for (const [y, wy] of ys)
        for (const [x, wx] of xs) {
          if (x < 0 || y < 0 || x >= image.width || y >= image.height) continue;
          const i = (y * image.width + x) * 4;
          const w = (wx * wy * image.data[i + 3]) / 255;
          r += image.data[i] * w;
          g += image.data[i + 1] * w;
          b += image.data[i + 2] * w;
          a += w;
        }
      const o = (oy * size + ox) * 4;
      const alpha = a / (scale * scale);
      if (alpha * 255 < 0.5) continue;
      out[o] = Math.round(r / a);
      out[o + 1] = Math.round(g / a);
      out[o + 2] = Math.round(b / a);
      out[o + 3] = Math.round(alpha * 255);
    }
  }
  return { width: size, height: size, data: out };
}

// Цвет свечения за значком: оттенок, которого в картинке больше всего. Берутся
// заметные (альфа от 1/4), насыщенные (от 0,35) и не тёмные пиксели — без
// чёрного и белого; вес — альфа. Оттенки — 36 корзин по 10°, побеждает окно из
// трёх соседних (±10°): цвет, разделённый границей корзин (золото у 10 — 20° и
// 30°), не проигрывает. Цвет — средний по ярко окрашенной части окна
// (насыщенность от 0,6: бледные блики сделали бы золото персиковым), доведённый
// до полной яркости, как у соседних значков (--glow: 40, 140, 255). Ответ —
// строка «r, g, b» для CSS.
function dominantGlow({ width, height, data }) {
  const bins = Array.from({ length: 36 }, () => ({ weight: 0, core: 0, r: 0, g: 0, b: 0 }));
  for (let i = 0; i < width * height * 4; i += 4) {
    const [r, g, b, a] = [data[i], data[i + 1], data[i + 2], data[i + 3]];
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    const saturation = max ? (max - min) / max : 0;
    if (a < 64 || max < 90 || saturation < 0.35) continue;
    const d = max - min;
    let hue = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    hue = (hue * 60 + 360) % 360;
    const bin = bins[Math.floor(hue / 10) % 36];
    const w = a / 255;
    bin.weight += w;
    if (saturation < 0.6) continue;
    bin.core += w;
    bin.r += r * w;
    bin.g += g * w;
    bin.b += b * w;
  }
  const sum = (k, key) => bins[(k + 35) % 36][key] + bins[k][key] + bins[(k + 1) % 36][key];
  let top = 0;
  for (let k = 1; k < 36; k++) if (sum(k, "weight") > sum(top, "weight")) top = k;
  if (!sum(top, "core")) return null;
  const mean = ["r", "g", "b"].map((key) => sum(top, key) / sum(top, "core"));
  const peak = Math.max(...mean);
  return mean.map((v) => Math.round((v * 255) / peak)).join(", ");
}

function main(dir) {
  const sources = [];
  for (let n = 1; n <= COUNT; n++) {
    const file = path.join(dir, `badge_${String(n).padStart(2, "0")}.png`);
    const bytes = fs.readFileSync(file);
    sources.push({ n, bytes, image: decodePng(bytes) });
  }
  // Общая рамка: объединение границ всех десяти, квадратная, с полем.
  const boxes = sources.map((s) => contentBox(s.image));
  const [x0, y0, x1, y1] = [0, 1, 2, 3].map((k) => (k < 2 ? Math.min : Math.max)(...boxes.map((box) => box[k])));
  const side = Math.max(x1 - x0, y1 - y0) * (1 + 2 * MARGIN);
  const left = (x0 + x1) / 2 - side / 2;
  const top = (y0 + y1) / 2 - side / 2;
  console.log(`рамка: ${side.toFixed(1)} px с (${left.toFixed(1)}, ${top.toFixed(1)}), содержимое ${x0},${y0}–${x1},${y1}`);

  fs.mkdirSync(OUT, { recursive: true });
  const glow = {};
  for (const s of sources) {
    const png = encodePng(resample(s.image, left, top, side, SIZE));
    fs.writeFileSync(path.join(OUT, `anniversary-${s.n}.png`), png);
    const sha = (buf) => require("crypto").createHash("sha256").update(buf).digest("hex");
    console.log(`anniversary-${s.n}.png ← badge_${String(s.n).padStart(2, "0")}.png | ${sha(s.bytes)} | ${sha(png)}`);
    if (s.n > 1) glow[s.n] = dominantGlow(decodePng(png));
  }
  console.log("GLOW = " + JSON.stringify(glow, null, 2));
}

if (require.main === module) {
  if (!process.argv[2]) {
    console.error("укажи папку с badge_01..10.png");
    process.exitCode = 2;
  } else main(process.argv[2]);
}

module.exports = { decodePng, encodePng, dominantGlow, resample, contentBox };
