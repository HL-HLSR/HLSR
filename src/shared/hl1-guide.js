"use strict";

// Скрипты гайда https://github.com/yavats/hl1-scripts для вкладки «Скрипты»
// Half-Life 1. Данные собирает scripts/import-hl1-guide.js из README гайда
// (зафиксированный коммит) в hl1-guide-data.json: alias скриптов уже
// с префиксами, чтобы скрипты разных авторов не перетирали друг друга.
//
// Скрипт назначается так же, как любая заготовка клавиатуры: на клавишу
// ставится одно из его действий, а alias лаунчер пишет в hlsr-binds.cfg,
// только пока хотя бы одно действие скрипта назначено.

const data = require("./hl1-guide-data.json");
// Перевод описаний (notes, usage) на русский по id скрипта. У каждого
// перевода хэш оригинала: после нового импорта гайда изменившийся текст
// показывается без перевода, пока перевод не обновят.
const translations = { ru: require("./hl1-guide-ru.json") };

// В alias вида «sensitivity 2.5» гайд просит вписать свою чувствительность.
// Импорт заменяет число на этот токен, а лаунчер перед запуском подставляет
// значение sensitivity из config.cfg игрока.
const SENSITIVITY_TOKEN = "{sensitivity}";
// Значение по умолчанию в Half-Life.
const DEFAULT_SENSITIVITY = "3";
// Так же для m_pitch: скрипты Xen обнуляют его на время трюка, а потом
// возвращают значение игрока, со знаком инверсии мыши.
const M_PITCH_TOKEN = "{m_pitch}";
const DEFAULT_M_PITCH = "0.022";

// Глобальный alias, который выполняет game.cfg при каждой загрузке карты
// или сохранения. Скрипты с needsAutoAlias меняют его перед сменой уровня.
const AUTO_ALIAS = "auto_alias";
const GAME_CFG = "game.cfg";

const byId = Object.create(null);
for (const script of data.scripts) byId[script.id] = script;

function scriptById(id) {
  return (typeof id === "string" && byId[id]) || null;
}

// Число из config.cfg или значение по умолчанию: в alias попадает только
// десятичное число, а не произвольный текст.
function cleanSensitivity(value) {
  const text = typeof value === "string" ? value.trim() : typeof value === "number" ? String(value) : "";
  return /^\d{1,3}(\.\d{1,6})?$/.test(text) && Number(text) > 0 ? text : DEFAULT_SENSITIVITY;
}

function cleanPitch(value) {
  const text = typeof value === "string" ? value.trim() : typeof value === "number" ? String(value) : "";
  return /^-?\d{1,3}(\.\d{1,6})?$/.test(text) && Number(text) !== 0 ? text : DEFAULT_M_PITCH;
}

function withSensitivity(value, sensitivity) {
  return String(value).split(SENSITIVITY_TOKEN).join(cleanSensitivity(sensitivity));
}

// В alias есть значение, которое лаунчер подставит из cfg игрока.
function hasPlayerToken(value) {
  return String(value).includes(SENSITIVITY_TOKEN) || String(value).includes(M_PITCH_TOKEN);
}

// Подставляет в alias значения игрока; cvars — переменные из его cfg.
function withPlayerCvars(value, cvars = {}) {
  return withSensitivity(value, cvars.sensitivity).split(M_PITCH_TOKEN).join(cleanPitch(cvars.m_pitch));
}

// Ссылки, которые вкладка «Скрипты» может открыть в браузере: страница
// гайда, сохранения и ссылки из описаний, видео скриптов на YouTube.
const LINKS = new Set([data.source.repo, `${data.source.repo}#readme`]);
const VIDEO_IDS = new Set();
for (const script of data.scripts) {
  if (script.save) LINKS.add(script.save);
  if (script.video) VIDEO_IDS.add(script.video.id);
  for (const text of [script.usage].concat(script.notes)) {
    for (const match of String(text || "").matchAll(/\]\((https:\/\/[^)\s]+)\)/g)) LINKS.add(match[1]);
  }
}
const WATCH_URL = /^https:\/\/www\.youtube\.com\/watch\?v=([\w-]{11})(?:&t=\d{1,6})?$/;

function isGuideLink(url) {
  if (typeof url !== "string") return false;
  if (LINKS.has(url)) return true;
  const watch = WATCH_URL.exec(url);
  return !!(watch && VIDEO_IDS.has(watch[1]));
}

// FNV-1a 32 бит по notes и usage: renderer считает его без node:crypto.
function textHash(script) {
  const text = JSON.stringify([script.notes, script.usage]);
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

// Описание скрипта на языке lang ({ notes, usage }) или null, если перевода
// нет или он сделан для другой версии текста.
function translationOf(script, lang) {
  const table = translations[lang];
  const item = table && script && table.scripts[script.id];
  if (!item || item.hash !== textHash(script) || item.notes.length !== script.notes.length) return null;
  return { notes: item.notes, usage: item.usage };
}

module.exports = {
  SOURCE: data.source,
  CHAPTERS: data.chapters,
  SCRIPTS: data.scripts,
  SENSITIVITY_TOKEN,
  DEFAULT_SENSITIVITY,
  M_PITCH_TOKEN,
  DEFAULT_M_PITCH,
  AUTO_ALIAS,
  GAME_CFG,
  scriptById,
  cleanSensitivity,
  cleanPitch,
  withSensitivity,
  hasPlayerToken,
  withPlayerCvars,
  isGuideLink,
  textHash,
  translationOf,
};
