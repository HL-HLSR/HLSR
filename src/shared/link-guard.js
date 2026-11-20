"use strict";

// Ссылки в описании профиля запрещены. Описание видят все, кто откроет
// профиль, и ссылка в нём — готовая реклама или фишинг. Сервер проверяет то
// же самое (server/backend/src/Profile/LinkGuard.php), здесь проверка нужна,
// чтобы сразу подсказать, а не ждать отказа сервера.
//
// Обходы, которые ловятся: http://, hxxp://, www., IP-адреса, punycode,
// «site.com», «site . com», «site [dot] com», «site (.) com», «site dot com»,
// «site точка ком», полноширинные и «китайские» точки, буквы через пробел
// («s i t e . c o m»), кириллица вместо похожих латинских букв («ѕіtе.соm»),
// невидимые символы между буквами и надстрочные знаки.

// Домены верхнего уровня, которые ловятся в записи без пробелов (site.com).
const TLDS = ("com net org edu gov info biz pro name mobi xyz top site online live link click club shop store app dev " +
  "page blog tech space fun icu buzz win bid stream chat social group team zone world today news media email cloud " +
  "host website web run games game wiki ink ly gg tv me io co cc fm am ai sh ms la vc gl gd ws pw tk ml ga cf gq to " +
  "su ru ua by kz uz kg tj tm az am ge md lt lv ee pl cz sk hu ro bg rs hr si ba mk al gr tr il de at ch fr be nl lu " +
  "uk ie es pt it dk no se fi is us ca mx br ar cl pe ve jp cn hk tw kr in id my sg th vn ph au nz za eg ng ke " +
  "рф рус ру ком орг сайт онлайн москва дети").split(" ");
// Те, что ловятся и в записи с пробелами или словами вместо точки. Без
// коротких английских слов (me, in, it, to, …): «Hi. Me too» — не ссылка.
const STRONG = ("com net org info biz pro xyz top site online link club shop store app dev ly gg tv io cc fm ws pw tk " +
  "ml ga cf gq su ru ua kz рф рус ру ком орг сайт онлайн").split(" ");

// Кириллица и греческие буквы, неотличимые от латинских.
const HOMOGLYPHS = {
  а: "a", в: "b", е: "e", ё: "e", к: "k", м: "m", н: "h", о: "o", р: "p", с: "c", т: "t", у: "y", х: "x",
  і: "i", ї: "i", ј: "j", ѕ: "s", ԁ: "d", ԛ: "q", ԝ: "w", ү: "y", һ: "h", ɡ: "g", ı: "i",
  α: "a", β: "b", ε: "e", η: "n", ι: "i", κ: "k", ν: "v", ο: "o", ρ: "p", τ: "t", υ: "u", χ: "x", ω: "w",
};

const alternation = (list) => list.map((tld) => tld.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
// Буква любого алфавита: «sité.com» — тоже ссылка.
const WORD = "[\\p{L}\\p{N}]";
const END = "(?![\\p{L}\\p{N}]|-[\\p{L}\\p{N}])";
const PATTERNS = [
  // Схема: http://, hxxps://, steam:// и т. п.
  /(?:^|[^a-z0-9])[a-z][a-z0-9+.-]{1,15}\s*:\s*\/\s*\//u,
  /(?:^|[^a-z0-9])www\s*\./u,
  /xn--/u,
  /(?:^|[^0-9])\d{1,3}(?:\s*\.\s*\d{1,3}){3}(?![0-9])/u,
  // site.com, site.com/путь
  new RegExp(WORD + "\\.(?:" + alternation(TLDS) + ")" + END, "u"),
  // site . com, site .com, site. com (последнее — только «сильные» зоны)
  new RegExp(WORD + "\\s+\\.\\s*(?:" + alternation(TLDS) + ")" + END, "u"),
  new RegExp(WORD + "\\.\\s+(?:" + alternation(STRONG) + ")" + END, "u"),
  // Короткие адреса приглашений даже без точки: discord gg/xxx, t me/xxx.
  /(?:^|[^a-z0-9])(?:discord\s*\.?\s*gg|t\s*\.?\s*me|bit\s*\.?\s*ly|vk\s*\.?\s*cc|clck\s*\.?\s*ru)\s*\/\s*[a-z0-9_]/u,
];

// Стилизованные латинские буквы, которые NFKD не раскладывает или которые
// сервер без intl разложить не может: полноширинные, в кружках, в
// квадратах, «математические» (𝐬𝐢𝐭𝐞) и региональные индикаторы (🇸🇮🇹🇪).
// Та же таблица — в LinkGuard.php.
function plainLetter(cp) {
  const az = (n) => String.fromCharCode(97 + n);
  if (cp >= 0xff01 && cp <= 0xff5e) return String.fromCharCode(cp - 0xfee0);
  if (cp >= 0x24b6 && cp <= 0x24cf) return az(cp - 0x24b6);
  if (cp >= 0x24d0 && cp <= 0x24e9) return az(cp - 0x24d0);
  if (cp >= 0x1d400 && cp <= 0x1d6a3) return az((cp - 0x1d400) % 52 % 26);
  if (cp >= 0x1d7ce && cp <= 0x1d7ff) return String((cp - 0x1d7ce) % 10);
  if (cp >= 0x1f130 && cp <= 0x1f189) return az((cp - 0x1f130) % 26);
  if (cp >= 0x1f1e6 && cp <= 0x1f1ff) return az(cp - 0x1f1e6);
  return null;
}

function fold(text) {
  return Array.from(String(text), (ch) => plainLetter(ch.codePointAt(0)) || ch).join("")
    // «ﬁ» — «fi», надстрочные знаки отделяются от букв.
    .normalize("NFKD")
    // Невидимые символы и надстрочные знаки между буквами.
    .replace(/[\p{M}\p{Cf}\u115f\u1160\u3164\uffa0\u2800]/gu, "")
    .toLowerCase()
    // Точки-заменители: «。», «․», «·», «•», «[dot]», «(.)», « dot », «точка».
    .replace(/[\u3002\uff0e\uff61\u2024\u2027\u00b7\u2022\u2219\u22c5\u30fb]/gu, ".")
    .replace(/\s*[[({<]\s*(?:dot|точка|\.)\s*[\])}>]\s*/gu, ".")
    .replace(/(^|\s)(?:dot|точка|тчк)(?=\s)/gu, "$1.")
    .replace(/[\u2044\u2215\u29f8]/gu, "/");
}

// «s i t e . c o m» → «site . com»: одиночные буквы через пробел склеиваются.
function squeeze(text) {
  return text.replace(/(?:[\p{L}\p{N}] ){2,}[\p{L}\p{N}](?![\p{L}\p{N}])/gu, (run) => run.replace(/ /g, ""));
}

function latin(text) {
  return text.replace(/[\u0250-\u02af\u0131\u0370-\u052f]/gu, (ch) => HOMOGLYPHS[ch] || ch);
}

function containsLink(value) {
  if (typeof value !== "string" || value.trim() === "") return false;
  const folded = fold(value);
  const variants = new Set([folded, squeeze(folded), latin(folded), latin(squeeze(folded))]);
  for (const text of variants) {
    if (PATTERNS.some((pattern) => pattern.test(text))) return true;
  }
  return false;
}

module.exports = { containsLink };
