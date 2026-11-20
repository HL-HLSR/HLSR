// Слои сняты в Ghosting Renewed 3.9, d2_coast_09, 1600×900.
// Мир масштабируется вокруг центра, оружие берётся из двух соседних
// ракурсов X и перепроецируется по глубине из параллакса по X/Z.
// Полная сетка сохраняет поверхности на перепадах глубины; соседний
// ракурс даёт скрытые прежде части. Низ рук дополнен кадрами FOV 100.
// qa-out/gh-preview/build-x.py воспроизводит ассеты без запуска игры.
// HUD смешивается отдельно в линейном свете: R — свечение, G — тень,
// B — подложка. Эти маски и порядок смешивания не меняются.
import { defaultPersonalization } from './ghosting-personalization-defaults';
const { defaults } = require('../../../shared/ghosting-settings');
export const FRAME_WIDTH = 1600;
export const FRAME_HEIGHT = 900;
const UNIT = FRAME_HEIGHT / 480;
const CX = FRAME_WIDTH / 2, CY = FRAME_HEIGHT / 2;
const META = require('@/assets/launcher/ghosting-preview/meta.json');
const SCENES = META.scenes.slice().sort((a, b) => a - b);
const FILES = {};
SCENES.forEach((step) => { FILES['scene-' + step] = require(`@/assets/launcher/ghosting-preview/scene-${step}.webp`); });
const VIEWS = META.viewmodels;
VIEWS.forEach((view, i) => {
  FILES['view-' + i] = require(`@/assets/launcher/ghosting-preview/views/${i}.webp`);
  FILES['depth-' + i] = require(`@/assets/launcher/ghosting-preview/views/${i}-depth.png`);
});
const tan = (fov) => Math.tan(fov * Math.PI / 360);
// The engine's fov is for a 4:3 screen of the same height.
const focal = (fov) => FRAME_HEIGHT * 2 / 3 / tan(fov);
// Шаг сетки по цветному ракурсу, в пикселях.
const MESH = 6;
// The narrowest frame that still holds the whole view.
function frameFor(list, fov) { return list.find((step) => step >= fov - 0.01) || list[list.length - 1]; }
// The stock colour (gh_hud_color_enable 0), as the masks measured it: the
// scheme's 255 220 0.
const STOCK = { r: 255, g: 220, b: 0 };
// The weapon's own crosshair; gh_hud_color_affects_crosshair recolours it.
const DOTS = [787, 437, 812, 464];
const DOT_STOCK = { r: 255, g: 221, b: 66 };
// The quick-info arcs around the crosshair always take the HUD colour.
const QUICKINFO = [758, 418, 840, 482];
// box: everything the element draws; plate: what gh_*_align positions.
// side: the screen edge the element keeps to in HudLayout.res (xpos "16",
// "c-57", "r150"); on another aspect ratio the frame is wider or narrower
// and the element moves with that edge.
// labels/icons are the parts gh_hud_labels/gh_hud_icons take away. digits,
// reserve and caption place the text when gh_hud_font replaces the HUD font.
const ELEMENTS = [
  { id: 'timer', key: 'gh_timer', side: 'left', box: [78, 246, 261, 291], plate: [84, 252, 255, 285], value: '00:00:00.000', digits: [169, 276, 19, 'center'] },
  { id: 'health', key: 'gh_health', side: 'left', box: [24, 804, 227, 883], plate: [30, 810, 221, 877], labels: [[42, 846, 101, 863]], caption: ['HEALTH', 44, 859], value: '100', digits: [131, 865, 51] },
  { id: 'suit', key: 'gh_suit', side: 'left', box: [256, 804, 470, 883], plate: [262, 810, 464, 877], labels: [[274, 846, 314, 864]], caption: ['SUIT', 276, 859], value: '60', digits: [358, 865, 51] },
  { id: 'speed', key: 'gh_speedmeter', side: 'center', box: [688, 804, 913, 883], plate: [694, 810, 907, 877], labels: [[706, 846, 740, 864]], caption: ['UPS', 708, 859], value: '0', digits: [790, 865, 51] },
  { id: 'ammo', key: 'gh_ammo', side: 'right', moved: (222 - 150) * UNIT, box: [1178, 804, 1437, 883], plate: [1184, 810, 1431, 877], labels: [[1195, 846, 1244, 863]], icons: [[1193, 825, 1243, 842]], caption: ['AMMO', 1197, 859], value: '45', digits: [1268, 865, 51], reserve: ['162', 1371, 866, 27] },
  { id: 'ammo2', key: 'gh_ammo2', side: 'right', box: [1452, 804, 1576, 883], plate: [1458, 810, 1570, 877], labels: [[1469, 850, 1502, 867]], icons: [[1456, 827, 1511, 849]], caption: ['ALT', 1471, 863], value: '3', digits: [1527, 865, 51] },
];
// gh_hud_color_alpha 0..255 against the scheme's own alpha (-1), measured in
// the game: the glyphs grow in step with it (the stock alpha is about 100),
// the dark captions less so.
const STOCK_ALPHA = 99.6;

// HUD мода Condensed Animated HUD (custom-packs.js, condensed-hud), каким его
// рисует игра. Раскладка — из его scripts/hudlayout.res (единицы 640×480),
// цифры — шрифтом HudNumbers из его resource/clientscheme.res: Alte DIN 1451
// Mittelschrift 2, tall 36, scanlines 2 (HudNumbersSmall — tall 16, weight 1000).
// FgColor мода — 255 220 0 220, у стандартного HUD альфа 100. Подписи и значки
// игра рисует теми же шрифтами, что у стандартного HUD: их пиксели берутся из
// масок и переносятся на места мода. Таймер и спидометр — блоки Ghosting.
const at480 = (v) => v * UNIT;
const fromRight = (v) => FRAME_WIDTH - at480(v);
const fromCentre = (v) => CX + at480(v);
const panel = (x, y, w, h) => [Math.round(x), Math.round(at480(y)), Math.round(x + at480(w)), Math.round(at480(y + h))];
const CONDENSED_ALPHA = 220;
const CONDENSED = [
  // text: начало подписи в моде, stockText — у стандартного (оттуда переносятся подписи и значки).
  { id: 'health', plate: panel(at480(16), 426, 48, 42), text: [at480(20), at480(428)], stockText: [at480(24), at480(452)],
    digits: { x: at480(18), top: at480(434), tall: at480(36) } },
  { id: 'suit', plate: panel(at480(68), 426, 48, 42), text: [at480(72), at480(428)], stockText: [at480(148), at480(452)],
    digits: { x: at480(70), top: at480(434), tall: at480(36) } },
  { id: 'speed', plate: null, stockPlate: true, text: [fromCentre(-49), at480(452)], stockText: [fromCentre(-49), at480(452)],
    digits: { x: fromCentre(-7), top: at480(434), tall: at480(36) } },
  // HudAmmo мода остаётся на r170 и с вторичными патронами (его hudanimations.txt не двигает панель).
  { id: 'ammo', plate: panel(fromRight(170), 432, 50, 36), text: [fromRight(162), at480(456)], stockText: [fromRight(214), at480(452)],
    digits: { x: fromRight(134), top: at480(434), tall: at480(36) }, reserve: { x: fromRight(104), top: at480(450), tall: at480(16) } },
  // У HudAmmoSecondary мода BgColor 0 0 0 0: плашки нет, место для привязки — размер панели.
  { id: 'ammo2', plate: null, anchor: panel(fromRight(76), 432, 60, 36), text: [fromRight(64), at480(456)], stockText: [fromRight(68), at480(454)],
    digits: { x: fromRight(40), top: at480(434), tall: at480(36) } },
];
const SHADE_STEPS = [[0, 0.81], [32, 1.037], [64, 1.262], [128, 1.714], [192, 2.169], [255, 2.612]];

const color = (c) => `rgb(${c.r},${c.g},${c.b})`;
const number = (v, fallback = 0) => Number.isFinite(Number(v)) ? Number(v) : fallback;
const fontName = (name) => name && !/[\\/]|\.(ttf|otf)$/i.test(name) ? name.replace(/[";{}]/g, '') : '';
const toLinear = (v) => v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
const LINEAR = Float32Array.from({ length: 256 }, (_, i) => toLinear(i / 255));
const SRGB_STEPS = 4096;
const SRGB = Uint8ClampedArray.from({ length: SRGB_STEPS + 1 }, (_, i) => {
  const v = i / SRGB_STEPS; return Math.round(255 * (v <= 0.0031308 ? v * 12.92 : 1.055 * Math.pow(v, 1 / 2.4) - 0.055));
});
const linearColor = (c) => [LINEAR[c.r], LINEAR[c.g], LINEAR[c.b]];
const inside = (rects, x, y) => rects.some((r) => x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
function canvas(w = FRAME_WIDTH, h = FRAME_HEIGHT) { const c = document.createElement('canvas'); c.width = w; c.height = h; return c; }
// decode() unpacks the picture off the main thread. With onload alone the
// first drawImage does it, in the middle of the tab's slide.
function image(src) {
  const i = new Image(); i.src = src;
  return i.decode().then(() => i, () => { throw new Error('Preview image unavailable'); });
}
function shadeGain(alpha) {
  const i = SHADE_STEPS.findIndex(([step]) => alpha <= step);
  if (i <= 0) return SHADE_STEPS[0][1];
  const [a0, k0] = SHADE_STEPS[i - 1], [a1, k1] = SHADE_STEPS[i];
  return k0 + (k1 - k0) * (alpha - a0) / (a1 - a0);
}
// How far an element on that side moves in a frame of the given width.
const shiftOf = (side, width) => side === 'right' ? width - FRAME_WIDTH : side === 'center' ? Math.round((width - FRAME_WIDTH) / 2) : 0;
// Where gh_<element>_align and _x/_y put an element: the anchor places its
// plate against the screen edge or centre, the offsets are 640x480 units
// scaled by the screen height (measured in the game at 1600x900 and
// 1440x900). Once any of them is set the game places the element from its
// HudLayout position: the ammo panel then leaves "r222", where the stock
// HUD moves it for weapons with secondary ammo, for "r150" (moved).
function offset(s, prefix, plate, width = FRAME_WIDTH, side = 'left', moved = 0) {
  const anchor = number(s[prefix + '_align']), dx = number(s[prefix + '_x']), dy = number(s[prefix + '_y']);
  const w = plate[2] - plate[0], h = plate[3] - plate[1];
  let x = plate[0] + shiftOf(side, width), y = plate[1];
  if (anchor >= 1 && anchor <= 9) {
    x = [0, (width - w) / 2, width - w][(anchor - 1) % 3];
    y = [0, (FRAME_HEIGHT - h) / 2, FRAME_HEIGHT - h][Math.floor((anchor - 1) / 3)];
  } else if (dx || dy) x += moved;
  return { x: Math.round(x + dx * UNIT - plate[0]), y: Math.round(y + dy * UNIT - plate[1]) };
}

let layersPromise;
export function loadLayers() {
  if (!layersPromise) {
    const first = ['scene-' + frameFor(SCENES, defaults().fov_desired), 'view-10', 'depth-10'];
    layersPromise = Promise.all(first.map((name) => image(FILES[name]).then((img) => [name, img])).concat(image(require('@/assets/launcher/ghosting-preview/hud.png'))))
      .then((list) => new OverlayRenderer(new Map(list.slice(0, -1)), list[list.length - 1]));
    layersPromise.catch(() => { layersPromise = null; });
  }
  return layersPromise;
}

// An idle period between the frames of an animation lasts a few ms; a long
// one means nothing is moving. A step waits for that, or for 5 s at most.
const idle = () => new Promise((resolve) => {
  const until = performance.now() + 5000;
  const check = (deadline) => (deadline.timeRemaining() >= 40 || performance.now() > until ? resolve() : requestIdleCallback(check));
  requestIdleCallback(check);
});
let preparePromise;
// Does in idle time, one step per idle period, what the first frame needs:
// the images, the HUD masks, the WebGL context with the weapon (creating it
// blocks for about 150 ms) and one frame. Opening the preview right after
// costs a frame, not half a second in the middle of the tab's slide.
export function prepareLayers() {
  if (!preparePromise) {
    preparePromise = idle().then(loadLayers)
      .then((renderer) => idle().then(() => renderer.viewmodelGl()).then(idle).then(() => { renderer.render(canvas(), defaultPersonalization()); renderer.prepared = true; renderer.warmViews(10); return renderer; }));
    preparePromise.catch(() => { preparePromise = null; });
  }
  return preparePromise;
}

export class OverlayRenderer {
  constructor(images, hud) {
    this.images = images;
    this.pending = new Map();
    this.parts = new Map();
    this.viewmodel = null;
    this.onLayer = null;
    this.prepared = false;
    const sheet = canvas(), sc = sheet.getContext('2d'); sc.drawImage(hud, 0, 0);
    const data = sc.getImageData(0, 0, FRAME_WIDTH, FRAME_HEIGHT).data, size = FRAME_WIDTH * FRAME_HEIGHT;
    this.glow = new Float32Array(size); this.shade = new Float32Array(size); this.plate = new Float32Array(size);
    for (let i = 0; i < size; i++) {
      this.glow[i] = (data[i * 4] / 255) ** 2; this.shade[i] = (data[i * 4 + 1] / 255) ** 2; this.plate[i] = (data[i * 4 + 2] / 255) ** 2;
    }
  }
  // Other fields of view load when first needed; until then the nearest loaded
  // frame stands in, and onLayer asks for a new frame.
  layer(prefix, list, fov) {
    const step = frameFor(list, fov), name = prefix + '-' + step;
    if (this.images.has(name)) return { step, img: this.images.get(name) };
    if (!this.pending.has(name)) {
      this.pending.set(name, image(FILES[name]).then((img) => { this.images.set(name, img); if (this.onLayer) this.onLayer(); }).catch(() => {}));
    }
    const loaded = list.filter((other) => this.images.has(prefix + '-' + other));
    const near = loaded.find((other) => other >= fov) || loaded[loaded.length - 1];
    return { step: near, img: this.images.get(prefix + '-' + near) };
  }
  loadView(i) {
    const name = 'view-' + i;
    if (this.images.has(name)) return Promise.resolve();
    if (!this.pending.has(name)) {
      const task = Promise.all([image(FILES[name]), image(FILES['depth-' + i])]).then(([colour, depth]) => new Promise((resolve) => {
        // Загрузка текстуры и создание сетки выполняются отдельной задачей
        // в простое, чтобы не добавлять их к кадру перетаскивания.
        requestIdleCallback(() => {
          this.images.set(name, colour); this.images.set('depth-' + i, depth);
          this.viewmodelView(i);
          if (this.onLayer) this.onLayer();
          resolve();
        }, { timeout: 250 });
      })).finally(() => this.pending.delete(name));
      this.pending.set(name, task);
    }
    return this.pending.get(name);
  }
  warmViews(i) {
    if (this.warming === i) return;
    this.warming = i;
    // Соседи готовятся между анимациями, по одному. Остальные ракурсы
    // загружаются только при приближении ползунка к ним.
    (async () => {
      for (const d of [-1, 1, -2, 2]) {
        const n = i + d;
        if (n < 0 || n >= VIEWS.length) continue;
        await idle();
        if (this.warming !== i) return;
        await this.loadView(n);
        this.viewmodelView(n);
      }
    })().catch(() => {});
  }
  viewmodelMesh(i) {
    const img = this.images.get('view-' + i), map = this.images.get('depth-' + i);
    const { at, x: sourceX } = VIEWS[i], { fov, inv } = META.viewmodel;
    const f = focal(fov), w = img.width, h = img.height;
    const c = canvas(map.width, map.height), dc = c.getContext('2d', { willReadFrequently: true }); dc.drawImage(map, 0, 0);
    const depth = dc.getImageData(0, 0, c.width, c.height).data;
    const cols = Math.ceil((w - 1) / MESH) + 1, rows = Math.ceil((h - 1) / MESH) + 1;
    const points = new Float32Array(cols * rows * 5);
    for (let r = 0; r < rows; r++) for (let q = 0; q < cols; q++) {
      const x = Math.min(w - 1, q * MESH), y = Math.min(h - 1, r * MESH);
      const mx = Math.min(c.width - 1, Math.round(x / (w - 1) * (c.width - 1))), my = Math.min(c.height - 1, Math.round(y / (h - 1) * (c.height - 1)));
      const o = (my * c.width + mx) * 4;
      const d = 1 / (inv[0] + (depth[o] * 256 + depth[o + 1]) / 65535 * (inv[1] - inv[0]));
      const n = (r * cols + q) * 5;
      points[n] = (x + at[0] - CX) / f * d - sourceX; points[n + 1] = d; points[n + 2] = (CY - y - at[1]) / f * d;
      points[n + 3] = (x + .5) / w; points[n + 4] = (y + .5) / h;
    }
    const index = new Uint32Array((cols - 1) * (rows - 1) * 6);
    let k = 0;
    for (let r = 0; r < rows - 1; r++) for (let q = 0; q < cols - 1; q++) {
      const n = r * cols + q;
      index.set([n, n + 1, n + cols, n + 1, n + cols + 1, n + cols], k); k += 6;
    }
    return { points, index, img };
  }
  viewmodelView(i) {
    const vm = this.viewmodelGl();
    if (!vm || !this.images.has('view-' + i)) return null;
    if (vm.views.has(i)) return vm.views.get(i);
    const { gl } = vm, mesh = this.viewmodelMesh(i);
    const vertex = gl.createBuffer(), index = gl.createBuffer(), texture = gl.createTexture();
    gl.bindBuffer(gl.ARRAY_BUFFER, vertex); gl.bufferData(gl.ARRAY_BUFFER, mesh.points, gl.STATIC_DRAW);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, index); gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, mesh.index, gl.STATIC_DRAW);
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, true);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, mesh.img);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR); gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE); gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    const view = { vertex, index, texture, points: mesh.points, count: mesh.index.length };
    vm.views.set(i, view);
    // Ограничиваем и декодированные изображения, и память GPU.
    for (const [old, v] of vm.views) {
      if (vm.views.size <= 7) break;
      if (Math.abs(old - this.warming) <= 2 || old === i) continue;
      gl.deleteBuffer(v.vertex); gl.deleteBuffer(v.index); gl.deleteTexture(v.texture); vm.views.delete(old);
      this.images.delete('view-' + old); this.images.delete('depth-' + old);
    }
    return view;
  }
  viewmodelGl() {
    if (this.viewmodel !== null) return this.viewmodel;
    this.viewmodel = false;
    const target = canvas(), gl = target.getContext('webgl', { premultipliedAlpha: true, preserveDrawingBuffer: true, antialias: true, depth: true });
    if (!gl || !gl.getExtension('OES_element_index_uint')) return false;
    const shader = (type, src) => { const sh = gl.createShader(type); gl.shaderSource(sh, src); gl.compileShader(sh); return sh; };
    const program = gl.createProgram();
    gl.attachShader(program, shader(gl.VERTEX_SHADER, `
      attribute vec3 point; attribute vec2 uv; varying vec2 tex;
      uniform vec3 offset; uniform float f, hand, bob; uniform vec2 frame;
      void main() {
        vec3 p = point + offset; p.y = max(p.y, 1.0);
        vec2 screen = vec2(frame.x / 2.0 + f * p.x / p.y, frame.y / 2.0 - f * p.z / p.y + bob);
        screen.x = hand > 0.0 ? screen.x : frame.x - screen.x;
        tex = uv;
        gl_Position = vec4(screen.x / frame.x * 2.0 - 1.0, 1.0 - screen.y / frame.y * 2.0, clamp(p.y / 200.0, 0.0, 1.0) * 2.0 - 1.0, 1.0);
      }`));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, `
      precision mediump float; varying vec2 tex; uniform sampler2D image;
      void main() { vec4 c = texture2D(image, tex); if (c.a < 0.02) discard; gl_FragColor = c; }`));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return false;
    gl.useProgram(program);
    gl.uniform2f(gl.getUniformLocation(program, 'frame'), FRAME_WIDTH, FRAME_HEIGHT);
    gl.enable(gl.DEPTH_TEST); gl.depthFunc(gl.LESS);
    gl.viewport(0, 0, FRAME_WIDTH, FRAME_HEIGHT); gl.clearColor(0, 0, 0, 0);
    // Каждый ракурс сначала проходит свой depth test. Затем смешиваются
    // готовые изображения: иначе дальний и ближний треугольники одного
    // ракурса складывались бы и оставляли светлые пятна.
    const compose = gl.createProgram();
    gl.attachShader(compose, shader(gl.VERTEX_SHADER, `attribute vec2 position; varying vec2 tex; void main() { tex = (position + 1.0) * 0.5; gl_Position = vec4(position, 0.0, 1.0); }`));
    gl.attachShader(compose, shader(gl.FRAGMENT_SHADER, `precision mediump float; varying vec2 tex; uniform sampler2D image; uniform float weight; void main() { gl_FragColor = texture2D(image, tex) * weight; }`));
    gl.linkProgram(compose);
    if (!gl.getProgramParameter(compose, gl.LINK_STATUS)) return false;
    const quad = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
    const texture = gl.createTexture(); gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, FRAME_WIDTH, FRAME_HEIGHT, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR); gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE); gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    const framebuffer = gl.createFramebuffer(); gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
    const depth = gl.createRenderbuffer(); gl.bindRenderbuffer(gl.RENDERBUFFER, depth);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT16, FRAME_WIDTH, FRAME_HEIGHT);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, depth);
    if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) return false;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    const at = (name) => gl.getUniformLocation(program, name);
    this.viewmodel = { gl, target, program, compose, quad, texture, framebuffer, position: gl.getAttribLocation(compose, 'position'), weight: gl.getUniformLocation(compose, 'weight'), views: new Map(), point: gl.getAttribLocation(program, 'point'), uv: gl.getAttribLocation(program, 'uv'), offset: at('offset'), f: at('f'), hand: at('hand'), bob: at('bob') };
    return this.viewmodel;
  }
  // Blends one part of the HUD into the frame the way the game does. source is
  // the part's box in the masks, at where it goes; glyph(x, y) says which of
  // its pixels keep their glyphs (in mask coordinates). Per pixel the game
  // does u * A + B in linear light, and A and B do not depend on the picture
  // underneath, so they are worked out once per look (key names the glyph
  // choice) and only the pixels the HUD touches are visited per frame.
  // Маски HUD мода (CONDENSED) в том же виде, что стандартные: таймер целиком,
  // подписи и значки — пиксели стандартных масок на местах мода и ярче в
  // CONDENSED_ALPHA / STOCK_ALPHA раз, плашки — стандартная плашка по 9 частям,
  // цифры — шрифтом мода (family; пусто — похожий DIN из Windows). Готовятся один
  // раз на шрифт.
  condensedMasks(family) {
    if (this.condensed && this.condensed.family === family) return this.condensed;
    const size = FRAME_WIDTH * FRAME_HEIGHT, k = CONDENSED_ALPHA / STOCK_ALPHA;
    const m = { key: 'condensed:' + family, family, alpha: CONDENSED_ALPHA, glow: new Float32Array(size), shade: new Float32Array(size), plate: new Float32Array(size), elements: [] };
    const visit = (rect, fn) => {
      for (let y = Math.max(0, rect[1]); y < Math.min(FRAME_HEIGHT, rect[3]); y++)
        for (let x = Math.max(0, rect[0]); x < Math.min(FRAME_WIDTH, rect[2]); x++) fn(x, y, y * FRAME_WIDTH + x);
    };
    const copyGlyphs = (rect, dx, dy, gain) => visit(rect, (x, y, i) => {
      const tx = x + dx, ty = y + dy;
      if (tx < 0 || ty < 0 || tx >= FRAME_WIDTH || ty >= FRAME_HEIGHT) return;
      const j = ty * FRAME_WIDTH + tx;
      m.glow[j] = Math.max(m.glow[j], Math.min(1, this.glow[i] * gain));
      m.shade[j] = Math.max(m.shade[j], Math.min(1, this.shade[i] * gain));
    });
    const stockOf = (id) => ELEMENTS.find((e) => e.id === id);
    // Плашка здоровья стандартного HUD: углы как есть, середина тянется.
    const source = stockOf('health').plate, corner = 10;
    const slice = (target) => visit(target, (x, y, i) => {
      const map = (v, t0, t1, p0, p1) => (v < t0 + corner ? p0 + (v - t0) : v >= t1 - corner ? p1 - (t1 - v) : p0 + corner);
      const sx = map(x, target[0], target[2], source[0], source[2]), sy = map(y, target[1], target[3], source[1], source[3]);
      m.plate[i] = Math.max(m.plate[i], this.plate[sy * FRAME_WIDTH + sx]);
    });
    // Яркость цифр — самая яркая точка цифр здоровья у стандартного HUD (шрифт тоже аддитивный).
    let peak = 0, peakShade = 0;
    visit([source[0] + 90, source[1], source[2], source[3]], (x, y, i) => { if (this.glow[i] > peak) { peak = this.glow[i]; peakShade = this.shade[i]; } });
    const probe = canvas(8, 8).getContext('2d');
    // tall в Source — высота ячейки GDI: winAscent + winDescent шрифта мода (1919 + 468 при 2048 на кегль).
    // Числа шрифта, а не замеры: запасной шрифт встаёт в ту же ячейку.
    const metrics = (tall, bold) => {
      const em = tall * 2048 / 2387;
      return { em, baseline: tall * 1919 / 2387, font: `${bold ? 'bold ' : ''}${em.toFixed(2)}px ${family ? `"${family}", ` : ''}"Bahnschrift SemiCondensed", "Arial Narrow", sans-serif` };
    };
    // Цифры в маску: scanlines 2 у HudNumbers мода — каждая вторая строка ячейки тусклее.
    const raster = (text, at, bold, scanlines) => {
      const { baseline, font } = metrics(at.tall, bold);
      probe.font = font;
      const pad = 4, w = Math.ceil(probe.measureText(text).width) + pad * 2, h = Math.ceil(at.tall) + pad * 2;
      const c = canvas(w, h), cc = c.getContext('2d', { willReadFrequently: true });
      cc.font = font; cc.fillStyle = '#fff'; cc.textBaseline = 'alphabetic'; cc.fillText(text, pad, pad + baseline);
      const data = cc.getImageData(0, 0, w, h).data, ox = Math.round(at.x) - pad, oy = Math.round(at.top) - pad;
      for (let r = 0; r < h; r++) for (let q = 0; q < w; q++) {
        let a = data[(r * w + q) * 4 + 3] / 255;
        if (!a) continue;
        if (scanlines && (r - pad) % 2) a *= 0.5;
        const tx = ox + q, ty = oy + r;
        if (tx < 0 || ty < 0 || tx >= FRAME_WIDTH || ty >= FRAME_HEIGHT) continue;
        const j = ty * FRAME_WIDTH + tx;
        m.glow[j] = Math.max(m.glow[j], Math.min(1, a * peak * k));
        m.shade[j] = Math.max(m.shade[j], Math.min(1, a * peakShade * k));
      }
      return [ox, oy, ox + w, oy + h];
    };
    // Для gh_hud_font: где шрифт игрока пишет цифры (базовая линия и кегль ячейки мода).
    const fontPlace = (at, align) => { const { em, baseline } = metrics(at.tall, false); return [Math.round(at.x), Math.round(at.top + baseline), Math.round(em), align]; };
    const union = (rects) => [Math.min(...rects.map((r) => r[0])) - 2, Math.min(...rects.map((r) => r[1])) - 2, Math.max(...rects.map((r) => r[2])) + 2, Math.max(...rects.map((r) => r[3])) + 2];

    const timer = stockOf('timer');
    visit(timer.box, (x, y, i) => { m.glow[i] = this.glow[i]; m.shade[i] = this.shade[i]; m.plate[i] = this.plate[i]; });
    m.elements.push(timer);
    for (const def of CONDENSED) {
      const stock = stockOf(def.id);
      const dx = Math.round(def.text[0] - def.stockText[0]), dy = Math.round(def.text[1] - def.stockText[1]);
      const move = (r) => [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];
      (stock.labels || []).concat(stock.icons || []).forEach((r) => copyGlyphs(r, dx, dy, k));
      const labels = (stock.labels || []).map(move), icons = (stock.icons || []).map(move);
      let plate = def.plate;
      if (def.stockPlate) { plate = stock.plate; visit(stock.box, (x, y, i) => { m.plate[i] = this.plate[i]; }); } else if (plate) slice(plate);
      const parts = [plate || def.anchor].concat(labels, icons, [raster(stock.value, def.digits, false, true)]);
      if (def.reserve && stock.reserve) parts.push(raster(stock.reserve[0], def.reserve, true, false));
      m.elements.push({
        id: stock.id, key: stock.key, side: stock.side, plate: plate || def.anchor, anchor: def.anchor, box: union(parts),
        labels, icons, caption: stock.caption && [stock.caption[0], stock.caption[1] + dx, stock.caption[2] + dy],
        value: stock.value, digits: fontPlace(def.digits, 'left'),
        reserve: def.reserve && stock.reserve ? [stock.reserve[0]].concat(fontPlace(def.reserve, 'left').slice(0, 3)) : undefined,
      });
    }
    this.condensed = m;
    return m;
  }
  // masks — набор масок (стандартный HUD или HUD мода, condensedMasks).
  blend(ctx, source, at, { plate = false, glyph = () => true, key = '', colour, gain = 1, shade = 1, masks = this }) {
    const x0 = Math.max(0, source[0] + at.x), y0 = Math.max(0, source[1] + at.y);
    const x1 = Math.min(ctx.canvas.width, source[2] + at.x), y1 = Math.min(FRAME_HEIGHT, source[3] + at.y);
    if (x1 <= x0 || y1 <= y0) return;
    const id = [masks.key || 'stock', key, source, at.x, at.y, plate, colour, gain, shade].join('|');
    let part = this.parts.get(id);
    if (!part) {
      const width = x1 - x0, where = [], mul = [], add = [], c = colour;
      for (let y = y0; y < y1; y++) for (let x = x0; x < x1; x++) {
        const sx = x - at.x, sy = y - at.y, i = sy * FRAME_WIDTH + sx;
        const p = plate ? masks.plate[i] : 0, keep = masks.glow[i] > 0 && glyph(sx, sy);
        if (!p && !keep) continue;
        const g = keep ? masks.glow[i] * gain : 0;
        where.push(((y - y0) * width + (x - x0)) * 4);
        mul.push((1 - p) * (keep ? 1 - Math.min(1, masks.shade[i] * shade) : 1));
        add.push(c[0] * g, c[1] * g, c[2] * g);
      }
      part = { where: Int32Array.from(where), mul: Float32Array.from(mul), add: Float32Array.from(add) };
      if (this.parts.size > 96) this.parts.clear();
      this.parts.set(id, part);
    }
    if (!part.where.length) return;
    const frame = ctx.getImageData(x0, y0, x1 - x0, y1 - y0), px = frame.data, { where, mul, add } = part;
    for (let n = 0; n < where.length; n++) {
      const o = where[n], m = mul[n], a = n * 3;
      for (let ch = 0; ch < 3; ch++) px[o + ch] = SRGB[Math.min(SRGB_STEPS, Math.round((LINEAR[px[o + ch]] * m + add[a + ch]) * SRGB_STEPS))];
    }
    ctx.putImageData(frame, x0, y0);
  }
  render(target, value, section = 'hud', assets = {}, time = 0) {
    if (!target) return;
    // The HUD is blended pixel by pixel (getImageData). A canvas kept on the
    // GPU would be read back for every element, which made sliders lag;
    // willReadFrequently keeps this one in memory (the first getContext
    // call decides).
    const ctx = target.getContext('2d', { willReadFrequently: true }), s = Object.assign(defaults(), value.extra || {});
    ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = 'high';
    ctx.globalAlpha = 1; ctx.globalCompositeOperation = 'source-over';
    // The frames were taken at 16:9. The game keeps the vertical field of
    // view, so a narrower screen loses the sides of the world and a wider
    // one sees more of it; the part the frames lack is filled with a blurred
    // stretch of the scene. Everything tied to the centre moves by this much.
    const width = target.width, centre = shiftOf('center', width);
    const world = number(s.fov_desired, 75), scene = this.layer('scene', SCENES, world);
    const k = Math.min(4, tan(scene.step) / tan(world));
    if (width > FRAME_WIDTH) {
      ctx.save(); ctx.filter = 'blur(18px) brightness(0.55)';
      ctx.drawImage(scene.img, 0, 0, width, FRAME_HEIGHT); ctx.restore();
    }
    ctx.save(); ctx.translate(centre, 0);
    ctx.drawImage(scene.img, CX - CX * k, CY - CY * k, FRAME_WIDTH * k, FRAME_HEIGHT * k);
    this.drawWeapon(ctx, s, time);
    ctx.restore();
    const hudColor = value.hudEnabled ? value.hudColor : STOCK;
    const alpha = value.hudEnabled && value.hudAlpha >= 0 ? value.hudAlpha : -1;
    // Включён HUD-мод: его раскладка и маски, настройки персонализации — поверх, как в игре.
    const masks = assets.hudMod === 'condensed-hud' ? this.condensedMasks(assets.hudModFont || '') : this;
    (masks.elements || ELEMENTS).forEach((e) => this.drawElement(ctx, e, s, hudColor, alpha, assets, masks));
    this.drawReticle(ctx, value, hudColor, centre);
    if (s.gh_keyshud) this.drawKeys(ctx, s, time);
    this.drawMeters(ctx, s, assets);
  }
  // The weapon has its own field of view (viewmodel_fov), whatever the view's.
  // viewmodel_offset_x/y/z move it right, forward and up in camera space;
  // cl_righthand 0 mirrors the picture.
  drawWeapon(ctx, s, time) {
    const f = focal(number(s.viewmodel_fov, 54)), right = s.cl_righthand ? 1 : -1;
    const dx = Math.max(-5, Math.min(5, number(s.viewmodel_offset_x))), dy = number(s.viewmodel_offset_y), dz = number(s.viewmodel_offset_z);
    const bob = time > 0 && s.viewmodel_bobbing ? Math.sin(time * 7) * 5 : 0;
    const position = (dx + 5) * 2, lo = Math.floor(position), hi = Math.min(20, lo + 1), mix = position - lo;
    for (const i of new Set([lo, hi])) this.loadView(i).catch(() => {});
    this.warmViews(Math.round(position));
    let chosen = [[lo, 1 - mix], [hi, mix]].filter(([i, w]) => w > .001 && this.images.has('view-' + i));
    if (!chosen.length) {
      const loaded = VIEWS.map((v, i) => i).filter((i) => this.images.has('view-' + i));
      const near = loaded.sort((a, b) => Math.abs(a - position) - Math.abs(b - position))[0];
      if (near === undefined) return;
      chosen = [[near, 1]];
    }
    const sum = chosen.reduce((n, v) => n + v[1], 0);
    const vm = this.viewmodelGl();
    if (vm) {
      const { gl } = vm;
      const composite = chosen.length > 1;
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      gl.useProgram(vm.program);
      gl.uniform3f(vm.offset, dx, dy, dz); gl.uniform1f(vm.f, f); gl.uniform1f(vm.hand, right); gl.uniform1f(vm.bob, bob);
      let x0 = FRAME_WIDTH, y0 = FRAME_HEIGHT, x1 = 0, y1 = 0;
      for (const [i, weight] of chosen) {
        const view = this.viewmodelView(i);
        gl.bindFramebuffer(gl.FRAMEBUFFER, composite ? vm.framebuffer : null); gl.useProgram(vm.program);
        gl.enable(gl.DEPTH_TEST); gl.disable(gl.BLEND);
        if (composite) gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
        gl.bindBuffer(gl.ARRAY_BUFFER, view.vertex); gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, view.index); gl.bindTexture(gl.TEXTURE_2D, view.texture);
        gl.enableVertexAttribArray(vm.point); gl.vertexAttribPointer(vm.point, 3, gl.FLOAT, false, 20, 0);
        gl.enableVertexAttribArray(vm.uv); gl.vertexAttribPointer(vm.uv, 2, gl.FLOAT, false, 20, 12);
        gl.drawElements(gl.TRIANGLES, view.count, gl.UNSIGNED_INT, 0);
        if (composite) {
          gl.bindFramebuffer(gl.FRAMEBUFFER, null); gl.useProgram(vm.compose); gl.disable(gl.DEPTH_TEST);
          gl.bindBuffer(gl.ARRAY_BUFFER, vm.quad); gl.enableVertexAttribArray(vm.position); gl.vertexAttribPointer(vm.position, 2, gl.FLOAT, false, 0, 0);
          gl.bindTexture(gl.TEXTURE_2D, vm.texture); gl.uniform1f(vm.weight, weight / sum);
          gl.enable(gl.BLEND); gl.blendFunc(gl.ONE, gl.ONE); gl.drawArrays(gl.TRIANGLES, 0, 6);
          gl.disable(gl.BLEND);
        }
        // Читаем только рамку оружия, а не все 1600×900 пикселей.
        const p = view.points;
        for (let n = 0; n < p.length; n += 5) {
          const depth = Math.max(1, p[n + 1] + dy);
          let x = CX + f * (p[n] + dx) / depth;
          if (right < 0) x = FRAME_WIDTH - x;
          const y = CY - f * (p[n + 2] + dz) / depth + bob;
          x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y);
        }
      }
      x0 = Math.max(0, Math.floor(x0) - 2); y0 = Math.max(0, Math.floor(y0) - 2);
      x1 = Math.min(FRAME_WIDTH, Math.ceil(x1) + 2); y1 = Math.min(FRAME_HEIGHT, Math.ceil(y1) + 2);
      if (x1 > x0 && y1 > y0) ctx.drawImage(vm.target, x0, y0, x1 - x0, y1 - y0, x0, y0, x1 - x0, y1 - y0);
      return;
    }
    const i = chosen[0][0], { at, x } = VIEWS[i], img = this.images.get('view-' + i), d = 25, k = f / focal(META.viewmodel.fov) * d / Math.max(5, d + dy);
    ctx.save();
    if (right < 0) { ctx.translate(FRAME_WIDTH, 0); ctx.scale(-1, 1); }
    ctx.translate(CX + f * (dx - x) / Math.max(5, d + dy), CY - f * dz / Math.max(5, d + dy) + bob);
    ctx.scale(k, k); ctx.drawImage(img, at[0] - CX, at[1] - CY); ctx.restore();
  }
  drawElement(ctx, e, s, c, alpha, assets, masks = this) {
    if (e.id === 'timer' && !s.gh_timer || e.id === 'speed' && !s.gh_speedmeter) return;
    const at = offset(s, e.key, e.anchor || e.plate, ctx.canvas.width, e.side, e.moved);
    const plate = e.id === 'timer' ? s.gh_timer_bg : e.id === 'speed' ? s.gh_speedmeter_bg && s.gh_hud_bg : s.gh_hud_bg;
    const labels = s.gh_hud_labels && (e.id !== 'speed' || s.gh_speedmeter_label);
    const font = assets.fontFamily || fontName(s.gh_hud_font);
    // Маски сняты при альфе схемы: у HUD мода она своя (CONDENSED_ALPHA).
    const base = masks.alpha || STOCK_ALPHA;
    const gain = alpha >= 0 ? alpha / base : 1, shade = alpha >= 0 ? shadeGain(alpha) * STOCK_ALPHA / base : 1;
    // A font draws the text itself; the mask then only gives the icons.
    const icons = s.gh_hud_icons && e.icons || [], hidden = [].concat(!labels && e.labels || [], !s.gh_hud_icons && e.icons || []);
    const glyph = font ? (x, y) => inside(icons, x, y) : hidden.length ? (x, y) => !inside(hidden, x, y) : undefined;
    this.blend(ctx, e.box, at, { plate, glyph, key: [e.id, font ? 'font' : '', labels ? 1 : 0, s.gh_hud_icons ? 1 : 0].join(), colour: linearColor(c), gain, shade, masks });
    if (font) this.drawText(ctx, e, s, c, Math.min(1, gain), labels, font, at);
    if (e.id === 'timer' && s.gh_timer_mirror) {
      ctx.save(); ctx.translate(at.x, at.y); ctx.fillStyle = color(c); ctx.font = 'bold 10px Arial'; ctx.fillText('LIVESPLIT', e.plate[0] + 6, e.plate[1] - 5);
      if (s.gh_timer_mirror_splits) { ctx.fillStyle = '#78d88c'; ctx.font = `${s.gh_timer_mirror_splits_bold ? 'bold' : 'normal'} 14px Arial`; ctx.fillText('−00:01.247', e.plate[0] + 6, e.plate[3] + 17); }
      ctx.restore();
    }
  }
  // gh_hud_font replaces the HUD font; only then do the scale, weight and
  // outline settings change anything in the game.
  drawText(ctx, e, s, c, opacity, labels, font, at) {
    const scale = number(s.gh_hud_fontscale, 1), weight = s.gh_hud_fontweight > 0 ? s.gh_hud_fontweight : 400;
    const family = `"${font}", sans-serif`;
    ctx.save(); ctx.translate(at.x, at.y); ctx.globalAlpha = opacity; ctx.fillStyle = color(c); ctx.textBaseline = 'alphabetic';
    ctx.lineJoin = 'round'; ctx.strokeStyle = 'rgba(0,0,0,.85)';
    const text = (value, x, y, size, align = 'left') => {
      ctx.font = `${weight} ${Math.round(size * scale)}px ${family}`; ctx.textAlign = align;
      if (s.gh_hud_fontoutline) { ctx.lineWidth = Math.max(2, size * scale / 12); ctx.strokeText(value, x, y); }
      ctx.fillText(value, x, y);
    };
    if (labels && e.caption) text(e.caption[0], e.caption[1], e.caption[2], 12);
    text(e.value, e.digits[0], e.digits[1], e.digits[2], e.digits[3]);
    if (e.reserve) text(e.reserve[0], e.reserve[1], e.reserve[2], e.reserve[3]);
    ctx.restore();
  }
  drawReticle(ctx, v, hudColor, centre = 0) {
    const none = { x: centre, y: 0 }, dots = (x, y) => inside([DOTS], x, y);
    if (!v.hideQuickInfo) this.blend(ctx, QUICKINFO, none, { glyph: (x, y) => !dots(x, y), key: 'quickinfo', colour: linearColor(hudColor) });
    if (!v.crossEnabled || !v.crossHideDefault) this.blend(ctx, DOTS, none, { key: 'dots', colour: linearColor(v.hudEnabled && v.hudAffectsCrosshair ? v.hudColor : DOT_STOCK) });
    if (!v.crossEnabled) return;
    // The launcher writes the HUD colour into gh_cross_color when the HUD
    // colour is meant to cover the crosshair too.
    const c = v.hudEnabled && v.hudAffectsCrosshair ? v.hudColor : v.crossColor;
    const x = ctx.canvas.width / 2, y = FRAME_HEIGHT / 2;
    ctx.save(); ctx.globalAlpha = number(v.crossAlpha, 255) / 255;
    const thickness = number(v.crossThickness, 2), length = number(v.crossSize, 10), gap = Math.max(0, number(v.crossGap, 3)), outline = v.crossOutline ? number(v.crossOutlineThickness, 1) : 0;
    const rect = (xx, yy, w, h, rgb = c) => { if (outline) { ctx.fillStyle = '#000'; ctx.fillRect(xx - outline, yy - outline, w + outline * 2, h + outline * 2); } ctx.fillStyle = color(rgb); ctx.fillRect(xx, yy, w, h); };
    if (v.crossTopLine && length) rect(x - thickness / 2, y - gap - length, thickness, length);
    if (v.crossBottomLine && length) rect(x - thickness / 2, y + gap, thickness, length);
    if (v.crossLeftLine && length) rect(x - gap - length, y - thickness / 2, length, thickness);
    if (v.crossRightLine && length) rect(x + gap, y - thickness / 2, length, thickness);
    if (v.crossCircleRadius > 0) { ctx.beginPath(); ctx.arc(x, y, v.crossCircleRadius, 0, Math.PI * 2); if (outline) { ctx.lineWidth = thickness + outline * 2; ctx.strokeStyle = '#000'; ctx.stroke(); } ctx.lineWidth = thickness; ctx.strokeStyle = color(c); ctx.stroke(); }
    if (v.crossDotSize > 0) rect(x - v.crossDotSize / 2, y - v.crossDotSize / 2, v.crossDotSize, v.crossDotSize, v.crossDotColor || c);
    ctx.restore();
  }
  // The keys panel keeps its own grey look: the HUD colour does not reach it.
  // Without its background only the keys being pressed show.
  drawKeys(ctx, s, time) {
    const full = s.gh_keyshud_full;
    const size = (full ? s.gh_keyshud_full_size : s.gh_keyshud_size) * UNIT, gap = s.gh_keyshud_padding * UNIT;
    const rows = full ? ['ESC  F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12  Prt Scr Pau','` 1 2 3 4 5 6 7 8 9 0 - = BKSP  INS HOME PGUP  NUM / * -','TAB Q W E R T Y U I O P [ ] \\  DEL END PGDN  7 8 9 +','CAPS A S D F G H J K L ; \' ENTER       4 5 6 +','SHIFT Z X C V B N M , . / SHIFT     ↑    1 2 3 ENTER','CTRL WIN ALT SPACE SPACE SPACE SPACE ALT WIN MENU CTRL    ← ↓ →   0 0 . ENTER'] : [' W +u','A S D','Duck Jump LM RM'];
    const columns = full ? Math.max(...rows.map((row) => row.split(' ').length)) : 4;
    const box = [0, 0, columns * (size + gap) + (s.gh_keyshud_mouse || s.gh_keyshud_wheel ? size * 4 : 0), rows.length * (size + gap)];
    const pos = offset(s, 'gh_keyshud', box, ctx.canvas.width);
    const key = (label, x, y, w, h, pressed) => {
      if (!s.gh_keyshud_bg && !pressed) return;
      ctx.fillStyle = pressed ? 'rgba(235,235,235,.85)' : 'rgba(205,205,205,.6)'; ctx.fillRect(x, y, w, h);
      if (label) { ctx.fillStyle = '#2c2c2c'; ctx.fillText(label, x + w / 2, y + h / 2, w - 3); }
    };
    ctx.save(); ctx.translate(pos.x, pos.y); ctx.font = `bold ${Math.max(8, size * .42)}px Arial`; ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
    rows.forEach((row, yy) => row.split(' ').forEach((label, xx) => { if (label) key(label, xx * (size + gap), yy * (size + gap), size, size, time && (label === 'W' || label === 'Jump' || label === 'SPACE')); }));
    const mx = columns * (size + gap) + 4;
    if (s.gh_keyshud_wheel) ['M↑', 'M↓', 'M'].forEach((label, i) => key(label, mx, i * (size + gap), size, size, false));
    if (s.gh_keyshud_mouse) {
      const xx = mx + size + gap, side = size * 2.5;
      key('', xx, 0, side, side, false);
      const cx = xx + side / 2, cy = side / 2; ctx.fillStyle = '#fff'; ctx.fillRect(cx - 2, cy - 2, 4, 4);
      if (time) { const length = Math.min(size, s.gh_keyshud_mouse_scale * 3); ctx.strokeStyle = '#fff'; ctx.beginPath(); ctx.moveTo(cx, cy); ctx.lineTo(cx + Math.sin(time * 3) * length, cy + Math.cos(time * 3) * length); ctx.stroke(); }
    }
    ctx.restore();
  }
  // cl_showfps / cl_showpos as the engine prints them in the top right corner.
  drawMeters(ctx, s, assets) {
    // The engine prints them at a fixed distance from the right edge.
    const left = 1301 + shiftOf('right', ctx.canvas.width);
    const draw = (prefix, lines, y, step, c) => {
      const family = assets[prefix] || fontName(s[prefix + '_font']) || 'Verdana';
      const size = number(s[prefix + '_fontsize']) || 14;
      ctx.save(); ctx.font = `${s[prefix + '_fontweight'] || 700} ${size}px "${family}", Verdana, sans-serif`; ctx.textAlign = 'left'; ctx.textBaseline = 'alphabetic'; ctx.fillStyle = c;
      lines.forEach((line, i) => {
        const yy = y + i * Math.max(step, size + 6);
        if (s[prefix + '_fontoutline']) { ctx.lineWidth = 2; ctx.lineJoin = 'round'; ctx.strokeStyle = 'rgba(0,0,0,.9)'; ctx.strokeText(line, left, yy); }
        ctx.fillText(line, left, yy);
      });
      ctx.restore();
    };
    const fps = number(s.cl_showfps);
    if (fps > 0) draw('gh_showfps', [fps === 2 ? '292 fps [49, 303] 3.4 ms on maps/d2_coast_09.bsp' : '297 fps on maps/d2_coast_09.bsp'], 19, 0, '#3cff2c');
    if (s.cl_showpos) draw('gh_showpos', ['pos:  -182.08 8441.09 -703.97', 'ang:  8.98 -93.21 0.00', 'vel:  0.00'], 50, 25, '#fff');
  }
}
