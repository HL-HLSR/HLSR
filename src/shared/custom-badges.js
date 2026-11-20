"use strict";

// Свои значки из админки (server/backend/src/Profile/CustomBadges.php):
// подпись на двух языках, пояснение и иконка SVG. Сервер присылает их
// описание вместе с профилем.
//
// SVG сервер уже пересобрал из разрешённых элементов, но лаунчер ему не
// доверяет: показывает иконку только через <img> с data:-адресом, а в таком
// виде Chromium не выполняет в SVG скрипты и не загружает ничего из сети.
// Здесь отсеиваются документы, которые на SVG не похожи или слишком велики.
//
// themed — стилизованный значок: лаунчер рисует силуэт иконки (CSS-маска с
// тем же data:-адресом) цветом акцента темы, как встроенные значки.

const ID = /^c-[0-9a-f]{8}$/;
const MAX_SVG = 20480;

function text(value, max) {
  return typeof value === "string" ? value.trim().slice(0, max) : "";
}

function normalizeOne(raw) {
  if (!raw || typeof raw !== "object" || !ID.test(raw.id)) return null;
  const svg = typeof raw.svg === "string" ? raw.svg.trim() : "";
  if (!svg || svg.length > MAX_SVG || !/^<svg[\s>]/.test(svg)) return null;
  const label = raw.label && typeof raw.label === "object" ? raw.label : {};
  const note = raw.note && typeof raw.note === "object" ? raw.note : {};
  const ru = text(label.ru, 40);
  if (!ru) return null;
  return {
    id: raw.id,
    label: { ru, en: text(label.en, 40) || ru },
    note: { ru: text(note.ru, 160), en: text(note.en, 160) || text(note.ru, 160) },
    icon: "data:image/svg+xml;base64," + Buffer.from(svg, "utf8").toString("base64"),
    themed: raw.themed === true,
  };
}

function normalize(list) {
  const out = [];
  for (const raw of Array.isArray(list) ? list.slice(0, 32) : []) {
    const badge = normalizeOne(raw);
    if (badge && !out.some((item) => item.id === badge.id)) out.push(badge);
  }
  return out;
}

module.exports = { ID, MAX_SVG, normalize };
