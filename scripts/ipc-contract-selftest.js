"use strict";

// A renderer call to a channel that preload blocks, or that main never handles,
// fails only when someone clicks the feature. Check the contract statically.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const SRC = path.join(__dirname, "..", "src");

function sources(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) return entry.name === "node_modules" ? [] : sources(file);
    return /\.(js|vue)$/.test(entry.name) ? [{ file: path.relative(SRC, file), text: fs.readFileSync(file, "utf8") }] : [];
  });
}

const main = sources(path.join(SRC, "main"));
const renderer = sources(path.join(SRC, "renderer"));
const preload = main.find((source) => source.file === path.join("main", "preload.js"));
const list = /INVOKE_CHANNELS\s*=\s*new Set\(\[([\s\S]*?)\]\)/.exec(preload.text);
assert.ok(list, "preload.js must declare INVOKE_CHANNELS");
const allowed = new Set([...list[1].matchAll(/["']([^"']+)["']/g)].map((match) => match[1]));

// Handlers are registered by literal name or through a frozen CHANNELS table.
const tables = {};
sources(SRC).forEach(({ text }) => {
  for (const table of text.matchAll(/\b([A-Z][A-Za-z]*)\s*=\s*(?:Object\.freeze\()?\{([^{}]*)\}/g))
    for (const entry of table[2].matchAll(/(\w+)\s*:\s*["']([^"']+)["']/g)) (tables[table[1]] = tables[table[1]] || {})[entry[1]] = entry[2];
});
const handled = new Set();
// Exercise the dynamic registration instead of guessing its object keys.
require("../src/main/online-profile/ipc").bind({
  handle(channel) { handled.add(channel); },
}, {}, () => {});
require("../src/main/telemetry/news-ipc").bind({
  handle(channel) { handled.add(channel); },
}, {});
require("../src/main/telemetry/sponsors-ipc").bind({
  handle(channel) { handled.add(channel); },
}, {}, {});
main.forEach(({ text }) => {
  for (const match of text.matchAll(/\.handle\(\s*["'`]([^"'`$]+)["'`]/g)) handled.add(match[1]);
  for (const match of text.matchAll(/\.handle\(\s*([A-Z][A-Za-z]*)\.(\w+)/g)) {
    const channel = tables[match[1]] && tables[match[1]][match[2]];
    assert.ok(channel, `cannot resolve ${match[1]}.${match[2]}`);
    handled.add(channel);
  }
});

test("channels invoked by the renderer are allowed by preload and handled by main", () => {
  const problems = [];
  renderer.forEach(({ file, text }) => {
    // runMenuAction(channel, …) в LauncherShell вызывает ipcRenderer.invoke(channel).
    for (const match of text.matchAll(/(?:ipcRenderer\.invoke|runMenuAction)\(\s*["'`]([^"'`$]+)["'`]/g)) {
      if (!allowed.has(match[1])) problems.push(`${file}: "${match[1]}" is blocked by preload`);
      if (!handled.has(match[1])) problems.push(`${file}: "${match[1]}" has no handler in main`);
    }
  });
  assert.deepEqual(problems, []);
});

test("channels allowed by preload have a handler in main", () => {
  assert.deepEqual([...allowed].filter((channel) => !handled.has(channel)), []);
});

// Заготовки подсветки для настроек профиля (StylePresetPicker): канал
// разрешён preload, обрабатывается main и вызывается интерфейсом.
test("public-style-presets-get: allowed by preload, handled by main, invoked by the profile settings", () => {
  assert.ok(allowed.has("public-style-presets-get"), "preload");
  assert.ok(handled.has("public-style-presets-get"), "main");
  const callers = renderer.filter(({ text }) => /ipcRenderer\.invoke\(\s*["']public-style-presets-get["']/.test(text)).map(({ file }) => path.basename(file));
  assert.deepEqual(callers, ["RunnerSettings.vue"]);
});

// Эпизоды Ghosting: состояние, установка и удаление — каналы в preload и в CHANNELS доставки.
test("game-episode-*: allowed by preload and handled by main through CHANNELS", () => {
  for (const channel of ["game-episodes-status", "game-episode-install", "game-episode-remove"]) {
    assert.ok(allowed.has(channel), `preload: ${channel}`);
    assert.ok(handled.has(channel), `main: ${channel}`);
    assert.ok(Object.values(tables.CHANNELS).includes(channel), `CHANNELS: ${channel}`);
  }
});

// Полный экран плеера «Демо»: окно лаунчера не fullscreenable, main разрешает
// полный экран на время плеера и сообщает, когда окно из него вышло.
test("demo-player-fullscreen: allowed by preload, handled by main; its change event is allowed", () => {
  assert.ok(allowed.has("demo-player-fullscreen"), "preload");
  assert.ok(handled.has("demo-player-fullscreen"), "main");
  const events = /EVENT_CHANNELS\s*=\s*new Set\(\[([\s\S]*?)\]\)/.exec(preload.text);
  assert.ok(events && /["']demo-player-fullscreen-changed["']/.test(events[1]), "событие в EVENT_CHANNELS");
});
// Babel's transform-runtime turns a CommonJS file into an ES module, and
// webpack then makes its module.exports read-only: the bundle throws
// "Cannot assign to read only property 'exports'" at startup. Such files
// must be in the babel-loader exclude list of the main bundle.
test("CommonJS sources of main are bundled without babel", () => {
  // Babel превращал module.exports в свойство только для чтения, и тесты не
  // могли подменять зависимости. Сборка обходится без него совсем.
  for (const name of ["main", "renderer"]) {
    const config = require(`../.electron-vue/webpack.${name}.config.js`);
    const loaders = JSON.stringify(config.module.rules);
    assert.doesNotMatch(loaders, /babel-loader/, `${name}: babel-loader`);
  }
  const commonjs = main.filter(({ file, text }) => /\.js$/.test(file) && /\bmodule\.exports\b/.test(text));
  assert.ok(commonjs.length > 0);
  assert.deepEqual(commonjs.filter(({ text }) => /^\s*(import|export)\s/m.test(text)).map(({ file }) => file), []);
});
