"use strict";

// Значки годовщины в HLSR: anniversary-N — N лет с регистрации профиля (N до
// 10). Сервер считает их сам по дате регистрации и присылает в badges только
// текущий; картинки — src/renderer/assets/launcher/badges/anniversary-N.png.

const IDS = Array.from({ length: 10 }, (_, i) => "anniversary-" + (i + 1));

// Свечение за значком (--glow в ProfileModal) — преобладающий оттенок самой
// картинки, посчитан scripts/prepare-anniversary-badges.js. У 1 свечения нет.
const GLOW = Object.freeze({
  2: "48, 156, 255",
  3: "46, 185, 255",
  4: "47, 181, 255",
  5: "33, 214, 255",
  6: "66, 67, 255",
  7: "123, 65, 255",
  8: "180, 33, 255",
  9: "206, 39, 255",
  10: "255, 178, 85",
});

// Ключ подписи «N год/года/лет в HLSR» по правилам языка (ru или en).
function labelKey(count, lang) {
  const form = new Intl.PluralRules(lang).select(count);
  return "#L_RUNNER_ANNIVERSARY_" + (form === "one" ? "ONE" : form === "few" ? "FEW" : "MANY");
}

module.exports = { IDS, GLOW, labelKey };
