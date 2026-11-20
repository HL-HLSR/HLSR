"use strict";

// Окно «Спонсорство» (SponsorsModal.vue): чистые функции без Vue, чтобы их
// проверяли тесты (scripts/sponsors-selftest.js).
//
// Ответ public-sponsors-get бывает двух видов:
//  - новый сервер (version 2): { links, active, donors, past } — три раздела
//    окна и адреса кнопок Tribute и бота;
//  - старый сервер на PHP (version 1): { sponsors } — окно как в 0.4.7.

const SECTIONS = ["active", "donors", "past"];
// Ключи links сервера и support-open (sponsors-client.js, sponsors-ipc.js).
const SUPPORT_KEYS = ["subscribe", "donate", "bot"];
// Кнопки оплаты в окне. Обе ведут в бота (support-open открывает links.bot
// с ?start=subscribe|donate), поэтому показываются только при его адресе.
const PAY_KEYS = ["subscribe", "donate"];

function isLegacy(result) {
  return Boolean(result) && result.version === 1;
}

// Разделы окна по порядку: [{ key, entries }]. В v2 — active, donors, past
// без пустых; в v1 — один раздел legacy, даже пустой (по нему окно узнаёт
// старую разметку). Записи не фильтруются и не копируются: номер записи в
// разделе — тот же, что в ответе, по нему main открывает её ссылку
// (sponsor-open-link).
function sectionsOf(result) {
  if (!result || typeof result !== "object") return [];
  if (result.version === 1) return [{ key: "legacy", entries: Array.isArray(result.sponsors) ? result.sponsors : [] }];
  if (result.version !== 2) return [];
  return SECTIONS.filter((key) => Array.isArray(result[key]) && result[key].length).map((key) => ({ key, entries: result[key] }));
}

// Кнопки оплаты: «Подписка» и «Донат», если сервер прислал адрес бота.
// Оплата идёт через бота (он сначала привязывает Telegram к Steam), адреса
// Tribute из links окну не нужны.
function supportKeys(links) {
  if (!links || typeof links !== "object") return [];
  return Object.hasOwn(links, "bot") && typeof links.bot === "string" && links.bot !== "" ? PAY_KEYS.slice() : [];
}

// Имя записи: своё имя (у донатов — из админки или ник профиля, его
// подставляет сервер), иначе ник профиля. "" — скрытый ник: окно пишет
// «Аноним».
function entryName(entry) {
  if (!entry || typeof entry !== "object") return "";
  const own = typeof entry.name === "string" ? entry.name.trim() : "";
  if (own) return own;
  const persona = entry.profile && typeof entry.profile.personaName === "string" ? entry.profile.personaName.trim() : "";
  return persona;
}

// Значок подписчика у ника: подписка активна (subscriber карточки) и ник
// виден. У скрытого ника («Аноним») значка нет, как и подсветки.
function entrySubscriber(entry) {
  return entryName(entry) !== "" && Boolean(entry.profile) && entry.profile.subscriber === true;
}

// Что делает клик по нику: "profile" — профиль HLSR по link.steamId64 (в
// карточке /v2 SteamID нет), "url" — свой адрес через main, null — ник не
// кликается.
function entryAction(entry) {
  const link = entry && entry.link;
  if (!link || typeof link !== "object") return null;
  if (link.mode === "profile" && typeof link.steamId64 === "string" && link.steamId64) return "profile";
  if (link.mode === "url" && typeof link.url === "string" && link.url) return "url";
  return null;
}

// «Царь горы»: первый — на троне, крупнее всех; 2-е и 3-е — пьедестал;
// остальные — компактно. При равной сумме выше тот, кто поддержал раньше
// (порядок сервера): чтобы занять трон, нужно дать больше. Те же места — в
// предпросмотре админки (sponsors.tsx).
function sponsorTier(index) {
  if (index === 0) return "king";
  return index < 3 ? "podium" : "rest";
}

function amountText(amount, locale) {
  return amount.toLocaleString(locale) + " ₽";
}

// От миллиона — коротко («100 млн ₽»), иначе сумма не влезает в карточку;
// полная сумма — в подсказке (amountText).
function amountShort(amount, locale) {
  if (amount < 1000000) return amountText(amount, locale);
  return new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(amount) + " ₽";
}

module.exports = { SECTIONS, SUPPORT_KEYS, PAY_KEYS, isLegacy, sectionsOf, supportKeys, entryName, entrySubscriber, entryAction, sponsorTier, amountText, amountShort };
