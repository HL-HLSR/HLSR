"use strict";

// Снимок экрана лаунчера в текстовом виде. Функция pageSnapshot сериализуется
// и выполняется в рендерере, поэтому внутри неё нельзя обращаться к
// замыканиям этого модуля.
//
// Что можно измерить точно, измеряется здесь, в коде: контраст, обрезанный
// текст, непереведённые ключи, выход за край окна. Модели получают уже
// названные факты («контраст 1.3, ниже порога»), а не цвета и пиксели:
// с числами и hex-цветами они работают плохо.

function pageSnapshot(options) {
  const opts = options || {};
  const maxText = opts.maxText || 6000;
  const vw = window.innerWidth;
  const vh = window.innerHeight;

  const INTERACTIVE =
    'button, a[href], input, select, textarea, summary, [role="button"], [role="tab"], [role="switch"],' +
    ' [role="checkbox"], [role="radio"], [role="menuitem"], [role="menuitemradio"], [role="option"],' +
    ' [role="slider"], [role="combobox"], [role="link"], [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';

  function clean(text) {
    return String(text || "")
      .replace(/\s+/g, " ")
      .trim();
  }

  function isVisible(el) {
    const rect = el.getBoundingClientRect();
    if (rect.width < 2 || rect.height < 2) return false;
    if (rect.bottom <= 0 || rect.right <= 0 || rect.top >= vh || rect.left >= vw) return false;
    for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) < 0.05) return false;
    }
    return true;
  }

  // Элемент реально доступен для клика: точка в его центре не перекрыта
  // модалкой, оверлеем или соседним блоком.
  function hitTestable(el) {
    const rect = el.getBoundingClientRect();
    const x = Math.min(vw - 1, Math.max(0, rect.left + rect.width / 2));
    const y = Math.min(vh - 1, Math.max(0, rect.top + rect.height / 2));
    const hit = document.elementFromPoint(x, y);
    return !!hit && (hit === el || el.contains(hit) || hit.contains(el));
  }

  function labelledBy(el) {
    const ids = (el.getAttribute("aria-labelledby") || "").split(/\s+/).filter(Boolean);
    return ids.map((id) => clean((document.getElementById(id) || {}).innerText)).join(" ");
  }

  function accessibleName(el) {
    const direct =
      el.getAttribute("aria-label") ||
      labelledBy(el) ||
      (el.labels && el.labels[0] && clean(el.labels[0].innerText)) ||
      clean(el.innerText) ||
      el.getAttribute("title") ||
      el.getAttribute("placeholder") ||
      el.getAttribute("alt");
    if (direct) return clean(direct).slice(0, 120);
    const img = el.querySelector("img[alt]");
    if (img && img.alt) return clean(img.alt);
    const svgTitle = el.querySelector("svg title");
    if (svgTitle) return clean(svgTitle.textContent);
    const tip = el.closest("[title]");
    return tip ? clean(tip.getAttribute("title")).slice(0, 120) : "";
  }

  function roleOf(el) {
    const role = el.getAttribute("role");
    if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === "a") return "link";
    if (tag === "select") return "combobox";
    if (tag === "textarea") return "textbox";
    if (tag === "input") {
      const type = (el.getAttribute("type") || "text").toLowerCase();
      if (type === "checkbox") return "checkbox";
      if (type === "radio") return "radio";
      if (type === "range") return "slider";
      if (type === "button" || type === "submit") return "button";
      return "textbox";
    }
    return tag === "button" || tag === "summary" ? "button" : "clickable";
  }

  function stateOf(el) {
    const state = {};
    const disabled = el.disabled || el.getAttribute("aria-disabled") === "true" || el.closest("[inert]");
    if (disabled) state.disabled = true;
    const checked = el.getAttribute("aria-checked") || el.getAttribute("aria-pressed");
    if (checked) state.checked = checked;
    else if (el.type === "checkbox" || el.type === "radio") state.checked = String(el.checked);
    if (el.getAttribute("aria-selected")) state.selected = el.getAttribute("aria-selected");
    if (el.getAttribute("aria-expanded")) state.expanded = el.getAttribute("aria-expanded");
    if (el.getAttribute("aria-current")) state.current = el.getAttribute("aria-current");
    if ("value" in el && el.tagName !== "BUTTON" && el.value !== undefined && el.value !== "") {
      state.value = String(el.value).slice(0, 80);
    }
    if (el.getAttribute("aria-valuenow")) state.value = el.getAttribute("aria-valuenow");
    const cls = String(el.className && el.className.baseVal !== undefined ? el.className.baseVal : el.className);
    if (/(^|[\s_-])(active|selected|is-active|current|on|checked)($|[\s_-])/.test(cls)) state.activeClass = true;
    return state;
  }

  // Ближайший заголовок области: модалка, секция, вкладка. Помогает модели
  // и отчёту понять, где находится элемент.
  function regionOf(el) {
    const dialog = el.closest('[role="dialog"], [aria-modal="true"]');
    if (dialog) {
      const head = dialog.querySelector("h1, h2, h3");
      return clean(dialog.getAttribute("aria-label") || labelledBy(dialog) || (head && head.innerText)).slice(0, 60);
    }
    const scope = dialog || el.closest("section, aside, nav, header, main, [class*='panel'], [class*='sidebar']");
    if (!scope) return "";
    const heading = scope.querySelector('h1, h2, h3, [class*="title"], [class*="heading"]');
    const name = scope.getAttribute("aria-label") || (heading && clean(heading.innerText));
    return clean(name).slice(0, 60);
  }

  // --- контраст -----------------------------------------------------------
  function parseColor(value) {
    const m = String(value).match(/rgba?\(([^)]+)\)/);
    if (!m) return null;
    const parts = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
  }
  function blend(top, bottom) {
    const a = top.a + bottom.a * (1 - top.a);
    if (a <= 0) return { r: 0, g: 0, b: 0, a: 0 };
    const mix = (k) => (top[k] * top.a + bottom[k] * bottom.a * (1 - top.a)) / a;
    return { r: mix("r"), g: mix("g"), b: mix("b"), a };
  }
  function luminance(c) {
    const ch = [c.r, c.g, c.b].map((v) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
  }
  // Фон под элементом: слои от элемента вверх. Картинка или градиент на пути
  // делают оценку ненадёжной, такие места отмечаются как unknown.
  function backgroundOf(el) {
    const layers = [];
    for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.backgroundImage && style.backgroundImage !== "none") return null;
      if (style.backdropFilter && style.backdropFilter !== "none" && layers.length === 0) {
        // стекло: цвет зависит от того, что под ним
      }
      const color = parseColor(style.backgroundColor);
      if (color && color.a > 0) {
        layers.push(color);
        if (color.a >= 0.99) break;
      }
    }
    if (!layers.length) return null;
    let result = layers[layers.length - 1];
    for (let i = layers.length - 2; i >= 0; i--) result = blend(layers[i], result);
    return result.a >= 0.9 ? result : null;
  }
  function contrastOf(el) {
    const style = getComputedStyle(el);
    const fg = parseColor(style.color);
    const bg = backgroundOf(el);
    if (!fg || !bg) return null;
    const color = blend({ ...fg, a: fg.a * Number(style.opacity || 1) }, bg);
    const l1 = luminance(color);
    const l2 = luminance(bg);
    const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    const size = parseFloat(style.fontSize);
    const bold = Number(style.fontWeight) >= 600;
    return { ratio: Math.round(ratio * 100) / 100, large: size >= 24 || (bold && size >= 18.66) };
  }

  // --- сбор ---------------------------------------------------------------
  const issues = [];
  const pushIssue = (kind, detail) => {
    if (issues.length < 80) issues.push({ kind, ...detail });
  };

  // Интерактивные элементы: семантические плюс div/span с cursor:pointer
  // (Vue-обработчики в DOM не видны, курсор — лучший признак кликабельности).
  const candidates = new Set(document.querySelectorAll(INTERACTIVE));
  for (const el of document.querySelectorAll("body *")) {
    if (candidates.has(el)) continue;
    if (getComputedStyle(el).cursor !== "pointer") continue;
    const parent = el.parentElement && el.parentElement.closest(INTERACTIVE);
    if (parent) continue;
    if (el.parentElement && getComputedStyle(el.parentElement).cursor === "pointer") continue;
    candidates.add(el);
  }

  // Переключатель: <label> со скрытым checkbox/radio внутри. Кликают по
  // label, состояние и имя берутся у input, а подпись — из строки настройки.
  function switchInput(el) {
    if (el.tagName !== "LABEL") return null;
    return el.querySelector('input[type="checkbox"], input[type="radio"]');
  }
  function rowLabel(el) {
    const row = el.closest('[class*="row"], [class*="field"], [class*="option"], li');
    if (!row) return "";
    const label = row.querySelector('[class*="label"], [class*="title"], [class*="name"]');
    return clean(label ? label.innerText : "").slice(0, 120);
  }
  for (const el of Array.from(candidates)) {
    if (el.tagName === "INPUT" && el.closest("label") && candidates.has(el.closest("label"))) candidates.delete(el);
  }

  let counter = 0;
  for (const old of document.querySelectorAll("[data-qa-ref]")) old.removeAttribute("data-qa-ref");
  const elements = [];
  for (const el of candidates) {
    if (!isVisible(el)) continue;
    const reachable = hitTestable(el);
    const rect = el.getBoundingClientRect();
    const ref = `e${++counter}`;
    el.setAttribute("data-qa-ref", ref);
    const input = switchInput(el);
    const item = {
      ref,
      role: input ? (input.type === "radio" ? "radio" : "switch") : roleOf(el),
      name: input ? clean(input.getAttribute("aria-label") || accessibleName(el) || rowLabel(el)) : accessibleName(el),
      region: regionOf(el),
      box: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)],
    };
    const state = input ? stateOf(input) : stateOf(el);
    if (Object.keys(state).length) item.state = state;
    if (input && !input.getAttribute("aria-label") && !input.getAttribute("aria-labelledby") && !clean(el.innerText)) {
      // Подпись видна глазами в строке, но не связана с input: скринридер
      // и автотесты видят безымянный переключатель.
      pushIssue("switch-without-accessible-name", { ref, visibleLabel: item.name, region: regionOf(el) });
    }
    if (!reachable) item.covered = true;
    if (!item.name && item.role !== "textbox" && !input) {
      pushIssue("unnamed-control", { ref, role: item.role, region: item.region, box: item.box });
    }
    if (rect.right > vw + 1 || rect.bottom > vh + 1 || rect.left < -1 || rect.top < -1) {
      item.partlyOffscreen = true;
    }
    elements.push(item);
  }

  // Текст: видимые листовые блоки. По ним же проверяются переводы,
  // обрезка и контраст.
  const texts = [];
  const seen = new Set();
  const openDialogs = Array.from(document.querySelectorAll('[role="dialog"], [aria-modal="true"]')).filter(isVisible);
  const topDialog = openDialogs[openDialogs.length - 1] || null;
  const walker = document.createTreeWalker(topDialog || document.body, NodeFilter.SHOW_TEXT);
  let total = 0;
  while (walker.nextNode()) {
    const node = walker.currentNode;
    const text = clean(node.nodeValue);
    if (!text) continue;
    const el = node.parentElement;
    if (!el || seen.has(el) || !isVisible(el)) continue;
    seen.add(el);
    const own = clean(Array.from(el.childNodes).filter((n) => n.nodeType === 3).map((n) => n.nodeValue).join(" "));
    if (!own) continue;

    if (/#L_[A-Za-z0-9_]+/.test(own)) pushIssue("untranslated-key", { text: own.slice(0, 120) });
    if (/(^|\W)(undefined|NaN|null)(\W|$)|\[object Object\]|\{\{.*\}\}/.test(own)) {
      pushIssue("broken-value", { text: own.slice(0, 120) });
    }

    const style = getComputedStyle(el);
    if (el.scrollWidth > el.clientWidth + 2 && style.overflowX !== "visible" && el.clientWidth > 0) {
      const kind = style.textOverflow === "ellipsis" ? "truncated-with-ellipsis" : "clipped-text";
      pushIssue(kind, { text: own.slice(0, 120), region: regionOf(el) });
    }
    const contrast = contrastOf(el);
    if (contrast && contrast.ratio < (contrast.large ? 2.2 : 3)) {
      pushIssue("low-contrast", { text: own.slice(0, 80), ratio: contrast.ratio, region: regionOf(el) });
    }

    if (total < maxText) {
      texts.push(own.slice(0, 200));
      total += own.length;
    }
  }

  const dialogs = Array.from(document.querySelectorAll('[role="dialog"], [aria-modal="true"]'))
    .filter(isVisible)
    .map((el) => clean(el.getAttribute("aria-label") || labelledBy(el) || (el.querySelector("h1,h2,h3") || {}).innerText || "").slice(0, 80));
  const toasts = Array.from(document.querySelectorAll('[role="status"], [role="alert"], [class*="toast"]'))
    .filter(isVisible)
    .map((el) => clean(el.innerText).slice(0, 160))
    .filter(Boolean);

  const active = document.activeElement;
  return {
    url: location.hash,
    viewport: [vw, vh],
    title: document.title,
    lang: document.documentElement.lang || "",
    theme: (() => {
      try {
        return (JSON.parse(localStorage.getItem("hlsr-launcher-prefs")) || {}).launcherTheme || "hlsr";
      } catch (error) {
        return "hlsr";
      }
    })(),
    dialogs: Array.from(new Set(dialogs)),
    toasts: Array.from(new Set(toasts)).slice(0, 6),
    focused: active && active !== document.body ? { role: roleOf(active), name: accessibleName(active) } : null,
    textScope: topDialog ? "top-dialog" : "page",
    elements,
    texts,
    issues,
  };
}

// Отпечаток экрана для обходчика: набор подписей и ролей без координат,
// чтобы одинаковые экраны с разными часами или счётчиками совпадали.
function fingerprint(snapshot) {
  const parts = snapshot.elements
    .filter((el) => !el.covered)
    .map((el) => `${el.role}:${el.name.replace(/\d+/g, "#")}`)
    .sort();
  return `${snapshot.dialogs.join("|")}::${parts.join(",")}`;
}

module.exports = { pageSnapshot, fingerprint };
