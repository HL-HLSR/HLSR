"use strict";

// Местоимения в профиле выбираются из списка, свой текст написать нельзя.
// На сервере хранится id, подпись берётся из языка того, кто смотрит.
// Список дублируется в Profile/Customization.php (PRONOUNS).
const PRONOUNS = Object.freeze([
  { id: "he", ru: "он/его", en: "he/him" },
  { id: "she", ru: "она/её", en: "she/her" },
  { id: "they", ru: "они/их", en: "they/them" },
  { id: "he-they", ru: "он/они", en: "he/they" },
  { id: "she-they", ru: "она/они", en: "she/they" },
  { id: "any", ru: "любые", en: "any pronouns" },
  { id: "ask", ru: "спросите меня", en: "ask me" },
]);

const IDS = new Set(PRONOUNS.map((item) => item.id));

// Пустая строка — «не указано». Старые свободные значения (до 0.4) и всё,
// чего нет в списке, не показываются.
function normalizePronouns(value) {
  return typeof value === "string" && IDS.has(value) ? value : "";
}

function pronounLabel(value, locale) {
  const item = PRONOUNS.find((entry) => entry.id === value);
  if (!item) return "";
  return locale === "ru" ? item.ru : item.en;
}

function pronounList(locale) {
  return PRONOUNS.map((item) => ({ id: item.id, label: locale === "ru" ? item.ru : item.en }));
}

module.exports = { PRONOUNS, normalizePronouns, pronounLabel, pronounList };
