"use strict";

// Плитки скриншотов в карточке поста. Та же раскладка в предпросмотре
// админки (web/admin/src/sections/news-crop.tsx): один скриншот — во всю
// ширину по пропорциям выбранной области (от 4:3 до 21:9), несколько — в две
// колонки 16:9, при нечётном числе первый на всю ширину 2:1.

const FULL = Object.freeze({ x: 0, y: 0, w: 1, h: 1 });
const MIN_SIDE = 0.02;
const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

// Область {x, y, w, h} в долях картинки; неверная — null (картинка целиком).
function parseCrop(raw) {
  if (!raw || typeof raw !== "object") return null;
  const parts = ["x", "y", "w", "h"].map((key) => raw[key]);
  if (parts.some((value) => typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 1)) return null;
  const [x, y, w, h] = parts;
  if (w < MIN_SIDE || h < MIN_SIDE || x + w > 1.0001 || y + h > 1.0001) return null;
  return { x, y, w: Math.min(w, 1 - x), h: Math.min(h, 1 - y) };
}

function tileAspect(image, index, count) {
  if (count === 1) {
    const crop = image.crop || FULL;
    return clamp((crop.w * image.width) / (crop.h * image.height), 4 / 3, 21 / 9);
  }
  return count % 2 === 1 && index === 0 ? 2 : 16 / 9;
}

// Положение картинки в плитке: область покрывает плитку по центру, как
// object-fit: cover, только относительно области, а не всей картинки.
function tileImageStyle(image, aspect) {
  const crop = image.crop || FULL;
  let w = crop.w * image.width, h = crop.h * image.height, x = crop.x * image.width, y = crop.y * image.height;
  if (w / h > aspect) { const next = h * aspect; x += (w - next) / 2; w = next; }
  else { const next = w / aspect; y += (h - next) / 2; h = next; }
  const pct = (value) => `${+(value * 100).toFixed(4)}%`;
  return { width: pct(image.width / w), height: pct(image.height / h), left: pct(-x / w), top: pct(-y / h) };
}

module.exports = { parseCrop, tileAspect, tileImageStyle };
