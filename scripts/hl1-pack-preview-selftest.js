"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { hudCommands, crosshairLayer } = require("../src/shared/hl1-pack-preview");
const schema = require("../src/shared/goldsrc-settings");
const meta = require("../src/renderer/assets/launcher/hl1-preview/packs.json");
const baseMeta = require("../src/renderer/assets/launcher/hl1-preview/meta.json");
const weaponMeta = require("../src/renderer/assets/launcher/hl1-preview/meta-weapons.json");

test("все формы и цвета установщика имеют отдельный фрагмент настоящего спрайта", () => {
  assert.deepEqual(meta.shapes, schema.CHANGER_SHAPES);
  assert.deepEqual(meta.colors, schema.CHANGER_COLORS);
  const tiles = new Set();
  for (const changerShape of schema.CHANGER_SHAPES) for (const changerColor of schema.CHANGER_COLORS) {
    const layer = crosshairLayer({ packCrosshair: "crosshair-changer", changerShape, changerColor }, meta);
    assert.equal(layer.image, "cross-changer");
    tiles.add(layer.box.join(","));
    const [x, y, w, h] = layer.box;
    assert.ok(x >= 0 && y >= 0 && x + w <= 224 && y + h <= 288);
  }
  assert.equal(tiles.size, 63);
  assert.equal(crosshairLayer({ packCrosshair: "default" }, meta), null);
});

test("крупные цифры, броня и оба вида патронов остаются внутри кадра WON", () => {
  const commands = hudCommands(meta.hud);
  for (const item of commands) {
    const [width, height] = item.box ? item.box.slice(2) : item.fill;
    assert.ok(item.at[0] >= 0 && item.at[1] >= 0 && item.at[0] + width <= 1600 && item.at[1] + height <= 900);
  }
  for (const name of ["suit_full", "suit_empty", "ammo", "ammo2", "flash_full", "flash_empty"])
    assert.ok(commands.some((item) => item.name === name), name);
  assert.ok(commands.filter((item) => item.name === "number_0").length >= 4);
});

function rendererFixture() {
  const contexts = [];
  const canvas = () => {
    const context = { images: [], fills: [], save() {}, restore() {},
      drawImage(...args) { this.images.push(args); }, fillRect(...args) { this.fills.push({ color: this.fillStyle, args }); } };
    const result = { width: 1600, height: 900, getContext: () => context };
    contexts.push(context);
    return result;
  };
  const file = path.join(__dirname, "../src/renderer/components/Launcher/hl1-overlay-renderer.js");
  const source = fs.readFileSync(file, "utf8").replace(/^import .*;$/m, "").replace(/export /g, "") + "\nmodule.exports = { Hl1Renderer };";
  const module = { exports: {} };
  // Image: слой, который ещё не загружен, грузится «вечно» — тест видит запрос (pending).
  vm.runInNewContext(source, { module, document: { createElement: canvas }, Image: class { decode() { return new Promise(() => {}); } },
    crosshairPixels: () => [{ x: 800, y: 450, w: 1, h: 1, color: { r: 0, g: 255, b: 0 } }],
    require(name) {
      if (name.endsWith("packs.json")) return meta;
      if (name.endsWith("meta-weapons.json")) return weaponMeta;
      if (name.endsWith("meta.json")) return baseMeta;
      if (name.endsWith("goldsrc-settings")) return schema;
      if (name.endsWith("hl1-pack-preview")) return { hudCommands, crosshairLayer };
      return name;
    },
  });
  const images = new Map(["scene-90", "weapon-90-f0", "semi-90-f0", "hud", "hud-stock", "panels", "viewangles", "crosshair", "hud-ag", "cross-execut4ble", "cross-changer"].map(name => [name, { name, width: 1600, height: 900 }]));
  const renderer = new module.exports.Hl1Renderer(images), target = canvas();
  return { renderer, target, contexts, context: target.getContext("2d") };
}

test("настоящий рендер выбирает пак вместо стандартного HUD и меняет цвет его цифр", () => {
  const { renderer, target, context, contexts } = rendererFixture();
  renderer.render(target, { hudEnabled: true, hudColor: { r: 40, g: 150, b: 200 }, extra: { packHud: "ag-hud" } }, "hl1packs");
  assert.ok(!context.images.some(args => ["hud", "hud-stock"].includes(args[0].name)));
  assert.ok(contexts.some(c => c.images.some(args => args[0].name === "hud-ag")));
  assert.ok(contexts.some(c => c.fills.some(item => item.color === "rgb(40,150,200)")));
});

test("рендер прицела паков учитывает форму, цвет и скрытие стандартного прицела в BXT", () => {
  const { renderer, target, context } = rendererFixture();
  renderer.render(target, { extra: { packCrosshair: "crosshair-changer", changerShape: "3px", changerColor: "pink" } }, "hl1packs");
  const drawn = context.images.find(args => args[0].name === "cross-changer");
  assert.deepEqual(drawn.slice(1, 5), [164, 68, 24, 24]);
  context.images = [];
  renderer.render(target, { crossEnabled: true, crossHideDefault: true, extra: { packCrosshair: "crosshair-changer" } }, "hl1packs");
  assert.ok(!context.images.some(args => /^cross/.test(args[0].name || "")));
  context.images = [];
  renderer.render(target, { extra: { packCrosshair: "execut4ble-crosshair" } }, "hl1packs");
  assert.ok(context.images.some(args => args[0].name === "cross-execut4ble"));
});

test("оружие: кадр сдвига вперёд-назад — ближайший не ближе к камере, за ближней плоскостью не рисуется", () => {
  const { renderer, target, context } = rendererFixture();
  const drawnWeapon = () => context.images.filter(args => /^(weapon|semi)-/.test(args[0].name)).map(args => args[0].name);
  // Пока нужный слой грузится, рисуется уже загруженный 90/0; сам слой запрошен.
  renderer.render(target, { extra: { viewmodelForward: 13 } }, "hl1view");
  assert.ok(renderer.pending.has("weapon-90-f20"), "13 берёт кадр 20, а не 10");
  renderer.images.set("weapon-90-f20", { name: "weapon-90-f20", width: 300, height: 300 });
  context.images = [];
  renderer.render(target, { extra: { viewmodelForward: 13 } }, "hl1view");
  assert.deepEqual(drawnWeapon(), ["weapon-90-f20"]);
  for (const [forward, want] of [[0, 0], [-15, -10], [20, 20], [5, 10], [-20, -20]]) {
    // Нужный кадр уже загружен: рисуется именно он, а не запасной 90/0.
    const name = `weapon-90-f${want}`;
    if (weaponMeta.weapon[90][want]) renderer.images.set(name, { name, width: 300, height: 300 });
    context.images = [];
    renderer.render(target, { extra: { viewmodelForward: forward } }, "hl1view");
    assert.deepEqual(drawnWeapon(), weaponMeta.weapon[90][want] ? [name] : [], `сдвиг ${forward} -> кадр ${want}`);
  }
});
