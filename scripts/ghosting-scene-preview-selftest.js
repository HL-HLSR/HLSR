"use strict";
// Превью «Призраки и запись» и «Эффекты и спреи»: расчёты по исходникам мода
// и то, что оба раздела показывают сцену, а поля идут одним списком.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs"), path = require("path");
const { parse, babelParse } = require("vue/compiler-sfc");
const scene = require("../src/shared/ghosting-scene-preview");
const ghosting = require("../src/shared/ghosting-settings");

const settings = (extra = {}) => Object.assign(ghosting.defaults(), extra);

test("спрей: квадрат — степень двойки от 8 до 256 по длинной стороне, на стене сторона × gh_spray_scale", () => {
  // Пример из описания gh_spray_scale: 256 пикселей при 0.25 — 64 юнита.
  assert.deepEqual(scene.sprayLayout(0.25, 256, 256, 192), { texels: 256, units: 64, pixels: 64 });
  assert.equal(scene.sprayLayout(0.25, 1920, 1080, 192).texels, 256);
  assert.equal(scene.sprayLayout(0.25, 100, 40, 192).texels, 64);
  assert.equal(scene.sprayLayout(0.25, 5, 3, 192).texels, 8);
  assert.equal(scene.sprayLayout(0.25, 128, 128, 192).units, 32);
  // Предел квара 0.05…2; ширина стены в кадре — 192 юнита.
  assert.equal(scene.sprayLayout(9, 256, 256, 192).units, 512);
  assert.equal(scene.sprayLayout(0.01, 256, 256, 192).units, 12.8);
  assert.equal(scene.sprayLayout(0.25, 256, 256, 1600).pixels, 64 * 1600 / scene.SPRAY.wallUnits);
});

test("призрак: gh_opacity как альфа, ближе gh_opacity_range прозрачнее пропорционально расстоянию", () => {
  const distance = scene.ghostDistance();
  assert.ok(distance > 200, "по умолчанию призрак дальше стандартного gh_opacity_range");
  assert.equal(scene.ghostAlpha(75, 200), 75);
  assert.equal(scene.ghostAlpha(75, 1000), Math.trunc(distance * 75 / 1000));
  assert.equal(scene.ghostAlpha(75, 0), 75);
  assert.equal(scene.ghostAlpha(400, 0), 255);
  assert.equal(scene.ghostAlpha(0, 200), 0);
});

test("имя над призраком: Verdana 13 px в кадре 1080, своим цветом или цветом HUD, пропадает дальше gh_ghost_names_dist", () => {
  const own = scene.ghostName(settings(), null);
  assert.equal(own.text, "Ghost");
  assert.equal(own.size, 13);
  assert.equal(own.alpha, 180);
  assert.deepEqual(own.color, ghosting.defaults().gh_ghost_color);
  const hud = { r: 0, g: 255, b: 255 };
  assert.deepEqual(scene.ghostName(settings({ gh_ghost_names_own_color: false }), hud).color, hud);
  assert.deepEqual(scene.ghostName(settings({ gh_ghost_names_own_color: false }), null).color, { r: 255, g: 220, b: 70 });
  assert.equal(scene.ghostName(settings({ gh_ghost_names: false }), null), null);
  assert.equal(scene.ghostName(settings({ gh_name: "" }), null), null);
  assert.equal(scene.ghostName(settings({ gh_ghost_names_dist: 100 }), null), null);
  assert.equal(scene.ghostName(settings({ gh_ghost_names_dist: 0 }), null).alpha, 180);
  // Над головой: выше верхушки модели (73 юнита) на экране.
  assert.ok(own.y < scene.project([scene.GHOST.origin[0], scene.GHOST.origin[1], 73]).y);
});

test("след: только при gh_draw_trails и длине > 0, гаснет к хвосту, длиннее — ярче на том же месте", () => {
  assert.deepEqual(scene.trailPoints(settings({ gh_draw_trails: false })), []);
  assert.deepEqual(scene.trailPoints(settings({ gh_trail_length: 0 })), []);
  const five = scene.trailPoints(settings()), minute = scene.trailPoints(settings({ gh_trail_length: 60 }));
  assert.ok(five.length > 30);
  assert.equal(five[0].alpha, 75 / 255);
  for (let i = 1; i < five.length; i++) assert.ok(five[i].alpha < five[i - 1].alpha);
  assert.ok(minute[30].alpha > five[30].alpha);
  // Лента начинается у ног призрака, видимая половина — над землёй.
  const feet = scene.project(scene.GHOST.origin);
  assert.ok(Math.abs(five[0].x - feet.x) < 1e-6 && Math.abs(five[0].y - feet.y) < 1e-6);
  assert.ok(five[0].ey < five[0].y);
});

test("фон призрака размыт и затемнён на 20 %, призрак с именем по центру вертикального кадра, след виден", () => {
  assert.equal(scene.BACKDROP.dim, 0.2);
  assert.ok(scene.BACKDROP.blur > 0);
  const sprite = require("../src/renderer/assets/launcher/ghosting-preview/scene/ghost.json");
  const name = scene.ghostName(settings(), null), feet = sprite.y + sprite.height;
  for (const aspect of [2 / 3, 3 / 4, 0.9]) {
    const view = scene.ghostView(aspect), inside = (p) => p.x >= view.x && p.x <= view.x + view.width && p.y >= view.y && p.y <= view.y + view.height;
    assert.ok(Math.abs(sprite.x + sprite.width / 2 - (view.x + view.width / 2)) < 1e-9, "середина модели — середина кадра");
    assert.ok(sprite.x > view.x && sprite.x + sprite.width < view.x + view.width, "модель целиком по ширине");
    // Запас сверху над именем и снизу под ногами — не меньше 10 % высоты.
    assert.ok(name.y - name.size - view.y >= view.height * 0.1);
    assert.ok(view.y + view.height - feet >= view.height * 0.1);
    // Двухсекундный кусок следа весь в кадре.
    const trail = scene.trailPoints(settings()).slice(0, 60);
    assert.ok(trail.length === 60 && trail.every(inside));
  }
});

// Компоненты: константы модуля, computed и методы берутся из исходника.
function component(name) {
  const script = parse(fs.readFileSync(path.join(__dirname, `../src/renderer/components/Launcher/${name}.vue`), "utf8")).descriptor.script.content;
  const ast = babelParse(script, { sourceType: "module" });
  const consts = {}, functions = {};
  const visit = (node) => {
    if (!node || typeof node !== "object") return;
    if (node.type === "VariableDeclarator" && node.init && ["ArrayExpression", "NumericLiteral"].includes(node.init.type)) consts[node.id.name] = script.slice(node.init.start, node.init.end);
    if (node.type === "ObjectMethod" && !functions[node.key.name]) functions[node.key.name] = "function(" + node.params.map((param) => script.slice(param.start, param.end)).join(",") + ")" + script.slice(node.body.start, node.body.end);
    for (const value of Object.values(node)) {
      if (Array.isArray(value)) value.forEach(visit);
      else if (value && typeof value === "object") visit(value);
    }
  };
  visit(ast);
  const scope = Object.keys(consts), values = scope.map((c) => new Function("return " + consts[c])());
  const build = (key) => new Function(...scope, "return " + functions[key])(...values);
  // getters — computed (пересчитываются при каждом чтении), methods — методы.
  return (vm, getters, methods = []) => {
    getters.forEach((key) => { const fn = build(key); Object.defineProperty(vm, key, { get: () => fn.call(vm), configurable: true }); });
    methods.forEach((key) => { vm[key] = build(key).bind(vm); });
    return vm;
  };
}

test("«Призраки и запись»: превью справа, поля обеих колонок — одним списком в прежнем порядке", () => {
  const extras = component("GhostingSettingsExtras");
  const section = ghosting.SECTIONS.find((item) => item.id === "ghosts");
  const vm = extras({ schema: null, activeId: "ghosts", activeSection: section, isAvailable: () => true }, ["hasPreview", "fieldColumns"]);
  assert.equal(vm.hasPreview, true);
  assert.equal(vm.fieldColumns.length, 1);
  assert.deepEqual(vm.fieldColumns[0].map((field) => field.key), section.fields.map((field) => field.key));
  // Без превью разделы по-прежнему делятся на две колонки.
  const chat = ghosting.SECTIONS.find((item) => item.id === "chat");
  assert.equal(extras({ schema: null, activeId: "chat", activeSection: chat, isAvailable: () => true }, ["hasPreview", "fieldColumns"]).fieldColumns.length, 2);
});

test("превью: «Призраки» — сцена с призраком, «Эффекты и спреи» — стена со спреем, остальные — кадр с HUD", () => {
  const preview = component("GhostingSettingsPreview");
  const at = (section, variant = "ghosting") => preview({ section, variant, get hl1() { return variant === "hl1"; } }, ["scene"]).scene;
  assert.equal(at("ghosts"), "ghost");
  assert.equal(at("misc"), "spray");
  assert.equal(at("hud"), "");
  assert.equal(at("weapon"), "");
  assert.equal(at("misc", "hl1"), "");
});

test("масштаб, «Вписать» и «100%» работают в вертикальном кадре", () => {
  const viewport = component("GhostingPreviewViewport");
  const vm = viewport({ frameWidth: 600, frameHeight: 900, box: { width: 360, height: 540 }, zoom: 1, cx: 300, cy: 450, redraw() {} },
    ["fit", "maxZoom", "scale", "percent", "zoomed", "atFit", "actual", "canZoomIn", "canZoomOut", "stageStyle"], ["clamp", "setScale", "zoomBy", "actualSize", "fitView"]);
  assert.equal(vm.percent, 60);
  assert.ok(vm.atFit && vm.canZoomIn && !vm.canZoomOut);
  vm.zoomBy(1.25);
  assert.equal(vm.percent, 75);
  assert.ok(vm.zoomed && vm.canZoomOut);
  vm.actualSize();
  assert.ok(vm.actual);
  // Кадр остаётся под окном: центр не уходит за края.
  assert.ok(vm.cx >= 180 && vm.cx <= 420 && vm.cy >= 270 && vm.cy <= 630);
  vm.fitView();
  assert.ok(vm.atFit);
  assert.deepEqual([vm.cx, vm.cy], [300, 450]);
  // В режиме fill окно не навязывает пропорции, их задаёт хозяин кадра.
  assert.deepEqual(vm.stageStyle, { aspectRatio: "600 / 900" });
  vm.fill = true;
  assert.equal(vm.stageStyle, null);
});
