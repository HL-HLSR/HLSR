"use strict";

// Рамка аватара — свой CSS. Как и оформление ника, его выдаёт админка
// (server/backend/src/Profile/AvatarFrames.php) или выбирает поддержавший
// проект вместе с подсветкой ника (заготовки, см. nick-styles.js). Рисуется
// отдельным слоем позади аватара, чуть шире него (компонент AvatarFrame.vue), и CSS
// меняет только вид этого слоя: обводку, тень, градиент, маску, фильтр и
// анимацию, в том числе готовые frame-spin (вращает --frame-angle для
// conic-gradient(from var(--frame-angle), …)) и frame-hue из launcher.css.
// Размеры, положение и url() запрещены. Список свойств совпадает с сервером.

const cssDeclarations = require("./css-declarations");

const MAX_CSS = 600;

const PROPERTIES = [
  "border", "border-width", "border-style", "border-color", "border-image", "border-image-source",
  "border-image-slice", "border-image-width", "outline", "outline-width", "outline-style", "outline-color",
  "outline-offset", "box-shadow", "background", "background-color", "background-image", "background-size",
  "background-position", "background-repeat", "background-origin", "background-clip",
  "mask", "mask-image", "mask-composite", "-webkit-mask", "-webkit-mask-image", "-webkit-mask-composite",
  "filter", "opacity", "animation", "animation-name", "animation-duration", "animation-timing-function",
  "animation-iteration-count", "animation-direction", "animation-delay",
];

function sanitizeCss(value) {
  return cssDeclarations.sanitize(value, PROPERTIES, MAX_CSS);
}

// То, что пришло с сервера, — ещё раз через ту же проверку.
function normalize(raw) {
  if (!raw || typeof raw !== "object") return null;
  const css = sanitizeCss(raw.css || "") || "";
  return css ? { css } : null;
}

module.exports = { PROPERTIES, MAX_CSS, sanitizeCss, normalize };
