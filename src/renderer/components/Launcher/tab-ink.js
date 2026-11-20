// Подсветка выбранной вкладки (.l-ink в launcher.css) и направление смены
// содержимого (переходы l-slide-next / l-slide-prev). Общие для вкладок
// «Персонализации» и профиля.

// Положение подсветки под вкладкой el внутри полосы вкладок container
// (у неё position: relative): отступы слева и справа.
export function inkBox(container, el) {
  if (!container || !el || !container.clientWidth) return null;
  return {
    left: el.offsetLeft,
    right: container.clientWidth - el.offsetLeft - el.offsetWidth,
  };
}

// "next", если новая вкладка правее прежней, иначе "prev".
export function direction(ids, from, to) {
  return ids.indexOf(to) >= ids.indexOf(from) ? "next" : "prev";
}
