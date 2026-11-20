// Переход по первым буквам в выпадающем списке, как у системного <select>:
// буквы, набранные подряд, складываются в префикс; одна и та же буква
// повторно перебирает подходящие пункты по кругу.
export const TYPEAHEAD_RESET_MS = 700;

function normalize(text) {
  return String(text || "").toLocaleLowerCase().replace(/ё/g, "е").trim();
}

export function findTypeaheadIndex(items, query, currentIndex) {
  const wanted = normalize(query);
  if (!wanted || !Array.isArray(items) || !items.length) return -1;
  const repeated = wanted.length > 1 && wanted.split("").every((char) => char === wanted[0]);
  const prefix = repeated ? wanted[0] : wanted;
  // Новый префикс ищется с текущего пункта, повтор буквы — со следующего.
  const start = repeated || wanted.length === 1 ? currentIndex + 1 : Math.max(currentIndex, 0);
  for (let step = 0; step < items.length; step++) {
    const index = (((start + step) % items.length) + items.length) % items.length;
    if (normalize(items[index] && items[index].label).startsWith(prefix)) return index;
  }
  return -1;
}

export function isTypeaheadKey(event) {
  return event.key.length === 1 && event.key !== " " && !event.ctrlKey && !event.metaKey && !event.altKey;
}
