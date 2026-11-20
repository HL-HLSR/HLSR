"use strict";

// Описания скриптов гайда hl1-scripts — урезанный markdown GitHub: абзацы,
// списки с вложенностью, блоки ```кода```, `код`, **жирный**, *курсив*,
// ссылки и картинки из репозитория гайда. Весь текст экранируется, теги
// ставит только этот файл, поэтому результат безопасно вставлять как HTML.
//
// Ссылки не открываются сами: у них data-script (другой скрипт гайда) или
// data-href (внешняя https-ссылка), переход делает вкладка «Скрипты».

const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (ch) => ESCAPES[ch]);
}

// Картинки гайда лежат в его репозитории, путь — только простые символы.
function imageUrl(src, options) {
  if (!options.imageBase || !/^[\w./%-]+$/.test(src) || src.includes("..")) return null;
  return `${options.imageBase}/${src.replace(/^\.?\//, "")}`;
}

function link(label, href, options) {
  if (href.startsWith("#")) {
    const id = href.slice(1);
    if (options.scriptIds && !options.scriptIds.has(id)) return label;
    return `<a href="#" data-script="${escapeHtml(id)}">${label}</a>`;
  }
  if (/^https:\/\/[^\s"'<>]+$/.test(href)) return `<a href="#" data-href="${escapeHtml(href)}">${label}</a>`;
  return label;
}

const INLINE_SOURCE = /`([^`]+)`|\*\*(.+?)\*\*|(?:^|(?<=[\s(]))\*(?!\s)(.+?)\*(?=$|[\s.,;:!?)])|!\[([^\]]*)\]\(([^)\s]+)\)|\[([^\]]+)\]\(([^)\s]+)\)/g;

function inline(text, options) {
  // Свой экземпляр на вызов: жирный и курсив разбираются рекурсивно.
  const pattern = new RegExp(INLINE_SOURCE.source, "g");
  let out = "";
  let last = 0;
  let m;
  while ((m = pattern.exec(text))) {
    out += escapeHtml(text.slice(last, m.index));
    last = pattern.lastIndex;
    if (m[1] !== undefined) out += `<code>${escapeHtml(m[1])}</code>`;
    else if (m[2] !== undefined) out += `<strong>${inline(m[2], options)}</strong>`;
    else if (m[3] !== undefined) out += `<em>${inline(m[3], options)}</em>`;
    else if (m[5] !== undefined) {
      const url = imageUrl(m[5], options);
      out += url ? `<img src="${escapeHtml(url)}" alt="${escapeHtml(m[4])}" loading="lazy">` : "";
    } else out += link(inline(m[6], options), m[7], options);
  }
  return out + escapeHtml(text.slice(last));
}

const LIST_ITEM = /^(\s*)([-*]|\d+\.)\s+(.*)$/;

// markdown → HTML. options: { imageBase, scriptIds: Set }.
function render(markdown, options = {}) {
  const lines = String(markdown || "").replace(/\r\n/g, "\n").split("\n");
  const out = [];
  let paragraph = [];
  // Открытые списки: { tag, indent }.
  const lists = [];

  const flushParagraph = () => {
    if (paragraph.length) out.push(`<p>${paragraph.map((line) => inline(line, options)).join("<br>")}</p>`);
    paragraph = [];
  };
  const closeLists = (indent) => {
    while (lists.length && lists[lists.length - 1].indent >= indent) {
      out.push(`</li></${lists.pop().tag}>`);
    }
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      flushParagraph();
      closeLists(0);
      const code = [];
      i += 1;
      while (i < lines.length && !/^\s*```/.test(lines[i])) code.push(options.preserveCodeIndent ? lines[i] : lines[i].trim()), (i += 1);
      out.push(`<pre><code>${escapeHtml(code.join("\n"))}</code></pre>`);
      continue;
    }
    const heading = options.headings && /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading) {
      flushParagraph(); closeLists(0);
      const level = Math.min(6, heading[1].length + 2);
      out.push(`<h${level}>${inline(heading[2], options)}</h${level}>`);
      continue;
    }
    const item = LIST_ITEM.exec(line);
    if (item) {
      flushParagraph();
      const indent = item[1].replace(/\t/g, "  ").length;
      const tag = item[2].endsWith(".") ? "ol" : "ul";
      const top = lists[lists.length - 1];
      if (top && top.indent > indent) closeLists(indent + 1);
      const current = lists[lists.length - 1];
      if (current && current.indent === indent && current.tag === tag) {
        out.push("</li><li>");
      } else {
        if (current && current.indent === indent) closeLists(indent);
        const start = tag === "ol" && item[2] !== "1." ? ` start="${Number.parseInt(item[2], 10)}"` : "";
        out.push(`<${tag}${start}><li>`);
        lists.push({ tag, indent });
      }
      out.push(inline(item[3], options));
      continue;
    }
    if (!line.trim()) {
      flushParagraph();
      closeLists(0);
      continue;
    }
    // Строка с отступом под пунктом списка продолжает этот пункт.
    if (lists.length && /^\s+/.test(line)) {
      out.push(`<br>${inline(line.trim(), options)}`);
      continue;
    }
    closeLists(0);
    paragraph.push(line.trim());
  }
  flushParagraph();
  closeLists(0);
  return out.join("");
}

module.exports = { render, inline, escapeHtml };
