"use strict";

// Оформление ника. Его выдаёт админка (server/backend/src/Profile/
// NickStyles.php) или выбирает сам игрок, поддержавший проект: в настройках
// профиля — заготовка «подсветка ника + рамка» из /v2/public/style-presets
// (StylePresetPicker.vue). Стиль из админки главнее заготовки — это решает
// сервер, профиль приходит уже с итоговым оформлением. Бывает двух видов:
//  - готовый стиль из каталога — класс .nick--<id> в launcher.css;
//  - свой CSS — несколько объявлений «свойство: значение», которые
//    применяются к нику как style (CSS заготовок — тоже такой).
// Свой CSS проходит ту же проверку, что на сервере: только свойства,
// меняющие вид текста (цвет, градиент, тень, начертание, анимация), без
// url() и всего, что меняет размеры и положение. Так ник не может ни
// загрузить что-то из сети (и узнать IP зрителя), ни сломать вёрстку.

const cssDeclarations = require("./css-declarations");

const PRESETS = ["developer", "gold", "ghost"];

const MAX_CSS = 600;

const PROPERTIES = [
  "color", "background", "background-color", "background-image", "background-size", "background-position",
  "background-repeat", "background-clip", "-webkit-background-clip", "-webkit-text-fill-color",
  "-webkit-text-stroke", "text-shadow", "font-weight", "font-style", "letter-spacing", "text-transform",
  "text-decoration", "text-decoration-color", "filter", "animation", "animation-name", "animation-duration",
  "animation-timing-function", "animation-iteration-count", "animation-direction", "animation-delay",
];

// Свой CSS: нормализованная строка объявлений, "" для пустой или null,
// если хоть одно объявление не прошло проверку.
function sanitizeCss(value) {
  return cssDeclarations.sanitize(value, PROPERTIES, MAX_CSS);
}

// То, что пришло с сервера, — ещё раз через ту же проверку: лаунчер не
// доверяет данным только потому, что их отдал свой сервер.
function normalize(raw) {
  if (!raw || typeof raw !== "object") return null;
  const style = PRESETS.indexOf(raw.style) !== -1 ? raw.style : "";
  const css = sanitizeCss(raw.css || "") || "";
  return style || css ? { style, css } : null;
}

module.exports = { PRESETS, PROPERTIES, MAX_CSS, sanitizeCss, normalize };
