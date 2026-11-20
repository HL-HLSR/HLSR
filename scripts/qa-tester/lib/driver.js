"use strict";

// Действия тестировщика над живым лаунчером: клики мышью в центр элемента,
// клавиатура, ввод текста, прокрутка. Всё идёт через Input.* CDP, как от
// настоящего пользователя, а не через el.click(): так ловятся перекрытые
// оверлеем кнопки и элементы, которые реагируют только на mousedown.

const { pageSnapshot } = require("./snapshot");
const { delay } = require("./cdp");

const KEYS = {
  Escape: { code: "Escape", keyCode: 27 },
  Enter: { code: "Enter", keyCode: 13, text: "\r" },
  Tab: { code: "Tab", keyCode: 9 },
  Space: { key: " ", code: "Space", keyCode: 32, text: " " },
  ArrowDown: { code: "ArrowDown", keyCode: 40 },
  ArrowUp: { code: "ArrowUp", keyCode: 38 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
  Backspace: { code: "Backspace", keyCode: 8 },
  Home: { code: "Home", keyCode: 36 },
  End: { code: "End", keyCode: 35 },
};

const MODIFIERS = { Alt: 1, Control: 2, Meta: 4, Shift: 8 };

class Driver {
  constructor(app) {
    this.app = app;
    this.page = app.page;
    this.last = null;
  }

  async snapshot(options) {
    this.last = await this.page.evaluate(`(${pageSnapshot.toString()})(${JSON.stringify(options || {})})`);
    return this.last;
  }

  // Ждёт, пока DOM перестанет меняться (анимации, загрузка данных),
  // но не дольше timeoutMs: часы и счётчики онлайна меняются постоянно.
  async settle({ quietMs = 350, timeoutMs = 4000 } = {}) {
    await this.page.evaluate(
      `new Promise((resolve) => {
        let timer = null;
        const done = () => { observer.disconnect(); clearTimeout(hard); resolve(true); };
        const observer = new MutationObserver(() => { clearTimeout(timer); timer = setTimeout(done, ${quietMs}); });
        observer.observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
        timer = setTimeout(done, ${quietMs});
        const hard = setTimeout(done, ${timeoutMs});
      })`,
      { timeoutMs: timeoutMs + 5000 }
    );
  }

  // Поиск элемента по ref из последнего снимка или по описанию
  // { name, role, region, nth }. name — строка (подстрока без учёта
  // регистра) или RegExp.
  async find(target, { fresh = true } = {}) {
    if (fresh || !this.last) await this.snapshot();
    if (typeof target === "string" && /^e\d+$/.test(target)) {
      return this.last.elements.find((el) => el.ref === target) || null;
    }
    const spec = typeof target === "string" || target instanceof RegExp ? { name: target } : target;
    const matchText = (value, pattern) => {
      if (pattern === undefined) return true;
      if (pattern instanceof RegExp) return pattern.test(value || "");
      return String(value || "").toLowerCase().includes(String(pattern).toLowerCase());
    };
    const matches = this.last.elements.filter(
      (el) =>
        matchText(el.name, spec.name) &&
        (!spec.role || (Array.isArray(spec.role) ? spec.role.includes(el.role) : el.role === spec.role)) &&
        matchText(el.region, spec.region) &&
        (spec.includeCovered || !el.covered)
    );
    if (spec.exact) {
      const exact = matches.filter((el) => el.name.toLowerCase() === String(spec.name).toLowerCase());
      if (exact.length) return exact[spec.nth || 0] || null;
    }
    return matches[spec.nth || 0] || null;
  }

  async center(ref) {
    return this.page.evaluate(`(() => {
      const el = document.querySelector('[data-qa-ref="${ref}"]');
      if (!el) return null;
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    })()`);
  }

  async mouse(type, x, y, extra = {}) {
    await this.page.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, ...extra });
  }

  async click(target, { settle = true, button = "left", double = false } = {}) {
    const el = await this.find(target);
    if (!el) throw new Error(`Элемент не найден: ${describe(target)}`);
    const point = await this.center(el.ref);
    if (!point) throw new Error(`Элемент пропал перед кликом: ${describe(target)}`);
    await this.mouse("mouseMoved", point.x, point.y, { button: "none" });
    await this.mouse("mousePressed", point.x, point.y, { button });
    await this.mouse("mouseReleased", point.x, point.y, { button });
    if (double) {
      await this.mouse("mousePressed", point.x, point.y, { button, clickCount: 2 });
      await this.mouse("mouseReleased", point.x, point.y, { button, clickCount: 2 });
    }
    if (settle) await this.settle();
    return el;
  }

  async hover(target) {
    const el = await this.find(target);
    if (!el) throw new Error(`Элемент не найден: ${describe(target)}`);
    const point = await this.center(el.ref);
    await this.mouse("mouseMoved", point.x, point.y, { button: "none" });
    await delay(600);
    return el;
  }

  async clickAt(x, y) {
    await this.mouse("mouseMoved", x, y, { button: "none" });
    await this.mouse("mousePressed", x, y);
    await this.mouse("mouseReleased", x, y);
    await this.settle();
  }

  async press(combo, { settle = true } = {}) {
    const parts = combo.split("+");
    const name = parts.pop();
    const modifiers = parts.reduce((sum, mod) => sum | (MODIFIERS[mod] || 0), 0);
    const def = KEYS[name] || { code: `Key${name.toUpperCase()}`, keyCode: name.toUpperCase().charCodeAt(0), text: name };
    const key = def.key || name;
    const base = { key, code: def.code, windowsVirtualKeyCode: def.keyCode, nativeVirtualKeyCode: def.keyCode, modifiers };
    await this.page.send("Input.dispatchKeyEvent", { type: def.text && !modifiers ? "keyDown" : "rawKeyDown", ...base, text: modifiers ? undefined : def.text });
    await this.page.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
    if (settle) await this.settle();
  }

  async type(text, { into, clear = false } = {}) {
    if (into) await this.click(into, { settle: false });
    if (clear) {
      await this.press("Control+a", { settle: false });
      await this.press("Backspace", { settle: false });
    }
    await this.page.send("Input.insertText", { text });
    await this.settle();
  }

  async scroll(target, deltaY = 400) {
    let point = { x: this.last ? this.last.viewport[0] / 2 : 640, y: this.last ? this.last.viewport[1] / 2 : 360 };
    if (target) {
      const el = await this.find(target);
      if (el) point = await this.center(el.ref);
    }
    await this.mouse("mouseWheel", point.x, point.y, { button: "none", deltaX: 0, deltaY });
    await this.settle();
  }

  // Сжатый JPEG для Clef: Cloudflare оценивает размер запроса по длине
  // base64, поэтому картинка держится в пределах ~100 КБ.
  async screenshot({ quality = 70, maxWidth = 1280 } = {}) {
    const vp = this.last ? this.last.viewport : await this.page.evaluate("[innerWidth, innerHeight]");
    const scale = Math.min(1, maxWidth / vp[0]);
    const shot = await this.page.send("Page.captureScreenshot", {
      format: "jpeg",
      quality,
      clip: { x: 0, y: 0, width: vp[0], height: vp[1], scale },
      captureBeyondViewport: false,
    });
    return Buffer.from(shot.data, "base64");
  }

  // Настройки интерфейса (тема, акцент и пр.) живут в localStorage
  // рендерера; после записи страница перезагружается, как после рестарта.
  async setPrefs(patch) {
    await this.page.evaluate(`(() => {
      const key = "hlsr-launcher-prefs";
      let saved = {};
      try { saved = JSON.parse(localStorage.getItem(key)) || {}; } catch (error) {}
      localStorage.setItem(key, JSON.stringify(Object.assign(saved, ${JSON.stringify(patch)})));
    })()`);
    await this.reload();
  }

  async reload() {
    await this.page.send("Page.reload", { ignoreCache: false });
    await delay(1500);
    await this.page.evaluate(
      `new Promise((resolve) => {
        const start = Date.now();
        const tick = () => (document.querySelector('[role="tab"]') || Date.now() - start > 20000 ? resolve(true) : setTimeout(tick, 200));
        tick();
      })`,
      { timeoutMs: 30000 }
    );
    await this.settle();
  }

  async evaluate(expression) {
    return this.page.evaluate(expression);
  }

  events() {
    return this.app.events();
  }
}

function describe(target) {
  if (typeof target === "string") return target;
  if (target instanceof RegExp) return String(target);
  return JSON.stringify(target, (k, v) => (v instanceof RegExp ? String(v) : v));
}

module.exports = { Driver, describe };
