// Preview of the HL1 customization. Layers are frames of the real game
// (Half-Life WON 2005 + Bunnymod XT, c2a5, 1600x900). No game process is started.
//
// The game draws its HUD and the BXT panels by adding colour × sprite to the
// picture in plain sRGB values. hud.png is the HUD sprites as the game adds
// them in white (per channel), panels.png the BXT digits; so a frame is
// picture + colour × mask, exact to 1/255 against the game. hud-stock.png is
// what the stock HUD adds (its colour and fading alpha), crosshair.png the
// weapon's own crosshair: it keeps its colour and is blended over everything,
// the BXT crosshair included (alpha solved from frames over black and white).
// viewangles.png is the angles text
// (console font, drawn opaque in the console colour).
//
// The picture is taken at several fields of view: a view is the nearest wider
// frame, enlarged around the centre (exact for a perspective view). The weapon
// is taken in the game at each of those fields of view and at five
// bxt_viewmodel_ofs_forward steps (-20, -10, 0, 10, 20; frame with and without
// r_drawviewmodel, the difference is the weapon), so moving it forward shows the
// arms and the rest of the model as the game does. Its semi-transparent look
// (bxt_viewmodel_semitransparent) is additive, so it is stored as what it adds.
// The weapon frames were taken in the dark start of c2a5 and brought to the
// light of the canyon scene by one gain per channel.
//
// The BXT panels are in the frame at their default anchors and move from
// there; the angles text is re-set from its own glyphs for bxt_hud_precision.
import { crosshairPixels } from './hl1-crosshair';
const { defaults } = require('../../../shared/goldsrc-settings');
const META = require('@/assets/launcher/hl1-preview/meta.json');
const PACKS = require('@/assets/launcher/hl1-preview/packs.json');
const { hudCommands, crosshairLayer } = require('../../../shared/hl1-pack-preview');

export const FRAME_WIDTH = 1600;
export const FRAME_HEIGHT = 900;
const CX = FRAME_WIDTH / 2, CY = FRAME_HEIGHT / 2;
// The BXT panels and the HUD without a colour of their own.
const BXT_STOCK = { r: 255, g: 160, b: 0 };

const FILES = {
  'scene-70': require('@/assets/launcher/hl1-preview/scene-70.webp'),
  'scene-80': require('@/assets/launcher/hl1-preview/scene-80.webp'),
  'scene-90': require('@/assets/launcher/hl1-preview/scene-90.webp'),
  'scene-100': require('@/assets/launcher/hl1-preview/scene-100.webp'),
  'scene-110': require('@/assets/launcher/hl1-preview/scene-110.webp'),
  'scene-120': require('@/assets/launcher/hl1-preview/scene-120.webp'),
  'scene-130': require('@/assets/launcher/hl1-preview/scene-130.webp'),
  'scene-140': require('@/assets/launcher/hl1-preview/scene-140.webp'),
  hud: require('@/assets/launcher/hl1-preview/hud.png'),
  'hud-stock': require('@/assets/launcher/hl1-preview/hud-stock.png'),
  panels: require('@/assets/launcher/hl1-preview/panels.png'),
  viewangles: require('@/assets/launcher/hl1-preview/viewangles.png'),
  crosshair: require('@/assets/launcher/hl1-preview/crosshair.png'),
  'hud-ag': require('@/assets/launcher/hl1-preview/hud-ag-atlas.png'),
  'cross-execut4ble': require('@/assets/launcher/hl1-preview/cross-execut4ble.png'),
  'cross-changer': require('@/assets/launcher/hl1-preview/cross-changer-atlas.png'),
};
const SCENES = META.scenes;
// Слои оружия: weapons/<weapon|semi>-<fov>-f<сдвиг>.webp; в meta-weapons.json — где лежит
// каждый в кадре. Нет записи — в игре оружия при этом сдвиге не видно (за ближней плоскостью).
const WEAPON_META = require('@/assets/launcher/hl1-preview/meta-weapons.json');
const WEAPONS = WEAPON_META.fovs;
const FORWARD_STEPS = WEAPON_META.forward;
const weaponFile = (name) => require(`@/assets/launcher/hl1-preview/weapons/${name}.webp`);

const tan = (fov) => Math.tan(fov * Math.PI / 360);
const number = (v, fallback = 0) => Number.isFinite(Number(v)) ? Number(v) : fallback;
const css = (c) => `rgb(${c.r},${c.g},${c.b})`;
function canvas(w = FRAME_WIDTH, h = FRAME_HEIGHT) { const c = document.createElement('canvas'); c.width = w; c.height = h; return c; }
// decode() unpacks the picture off the main thread, not in the first drawImage.
function image(src) {
  const i = new Image(); i.src = src;
  return i.decode().then(() => i, () => { throw new Error('Preview image unavailable'); });
}

// What the game shows: default_fov is horizontal on the real screen;
// bxt_fix_widescreen_fov treats it as a 4:3 view widened to the screen.
export function worldFov(s) {
  const fov = number(s.fov, 90);
  if (!s.fixWidescreenFov) return fov;
  const wide = Math.atan(tan(fov) * 0.75 * FRAME_WIDTH / FRAME_HEIGHT) * 360 / Math.PI;
  return Math.max(10, Math.min(150, wide));
}
// bxt_viewmodel_fov 0 draws the weapon with the view's own field of view.
export function weaponFov(s) { return number(s.viewmodelFov) > 0 ? number(s.viewmodelFov) : worldFov(s); }
// The narrowest frame that still holds the whole view.
function frameFor(list, fov) { return list.find((step) => step >= fov - 0.01) || list[list.length - 1]; }

// bxt_viewmodel_ofs_forward: the nearest frame taken at the same or a farther
// step. A farther frame holds more of the weapon (the arms), so enlarging it to
// the wanted depth only crops it at the screen edge, as the game does; a nearer
// one shrunk would show where it was cut off.
export function forwardStep(forward) {
  const f = number(forward);
  const found = FORWARD_STEPS.find((step) => step >= f);
  return found === undefined ? FORWARD_STEPS[FORWARD_STEPS.length - 1] : found;
}
// What is left after the frame (between the steps) and bxt_viewmodel_ofs_right/up:
// the layer is flat, so it moves as if all of it stood at one depth, about 20
// units (the middle of the MP5 in the frame): an approximation, in the game
// nearer parts move more and farther ones less. Not nearer than the near
// plane, 4 units.
const WEAPON_DEPTH = 20, NEAR_PLANE = 4;
function weaponOffset(s, fov, step = 0) {
  const depth = Math.max(NEAR_PLANE, WEAPON_DEPTH + number(s.viewmodelForward)), focal = CX / tan(fov);
  return { scale: (WEAPON_DEPTH + step) / depth, x: number(s.viewmodelRight) * focal / depth, y: -number(s.viewmodelUp) * focal / depth };
}

// BXT puts a panel at anchor × screen plus its own offset (GetPosition in
// hud_custom.cpp), the anchor read as a float and the product truncated. The
// frame has the default anchors, so a panel moves by the difference.
const BASE = defaults();
const anchorAt = (percent, size) => Math.trunc(Math.fround(Math.fround(percent / 100) * size));
function panelShift(s, panel) {
  const x = panel + 'X', y = panel + 'Y';
  return [
    anchorAt(number(s[x], BASE[x]), FRAME_WIDTH) - anchorAt(BASE[x], FRAME_WIDTH),
    anchorAt(number(s[y], BASE[y]), FRAME_HEIGHT) - anchorAt(BASE[y], FRAME_HEIGHT),
  ];
}
// bxt_hud_jumpdistance: the HUD digits one row above the jump speed (offset
// -4 rows against -3, anchor 0.5 1). Before a jump it shows 0, as the jump
// speed does in the frame.
const NUMBER_HEIGHT = META.panels.speedometer[1] - META.panels.jumpspeed[1];

// The angles in the frame are exact floats: pitch 89 and yaw 257.51953125,
// the one float printed as 257.519531. std::fixed writes them with
// bxt_hud_precision digits, an exact tie going to the even digit.
function fixed(value, digits) {
  const [whole, fraction = ''] = value.split('.');
  const all = whole + fraction.padEnd(digits, '0'), cut = whole.length + digits;
  const kept = all.slice(0, cut).split(''), rest = all.slice(cut);
  if (rest[0] > '5' || (rest[0] === '5' && (/[1-9]/.test(rest.slice(1)) || kept[kept.length - 1] % 2 === 1))) {
    let i = kept.length - 1;
    while (i >= 0 && kept[i] === '9') kept[i--] = '0';
    if (i < 0) kept.unshift('1'); else kept[i] = String(Number(kept[i]) + 1);
  }
  const point = kept.length - digits;
  return kept.slice(0, point).join('') + (digits > 0 ? '.' + kept.slice(point).join('') : '');
}

// The HUD colour as the launcher writes it: the alpha goes into the colour.
export function hudColor(value) {
  if (!value.hudEnabled) return null;
  const c = value.hudColor || BXT_STOCK, a = number(value.hudAlpha, -1);
  if (!(a > 0)) return c;
  return { r: Math.round(c.r * a / 255), g: Math.round(c.g * a / 255), b: Math.round(c.b * a / 255) };
}

let layersPromise;
export function loadLayers() {
  if (!layersPromise) {
    layersPromise = Promise.all(['scene-90', 'weapon-90-f0', 'semi-90-f0', 'hud', 'hud-stock', 'panels', 'viewangles', 'crosshair', 'hud-ag', 'cross-execut4ble', 'cross-changer'].map((name) => image(FILES[name] || weaponFile(name)).then((img) => [name, img])))
      .then((list) => new Hl1Renderer(new Map(list)));
    layersPromise.catch(() => { layersPromise = null; });
  }
  return layersPromise;
}

export class Hl1Renderer {
  constructor(images) {
    this.images = images;
    this.pending = new Map();
    this.onLayer = null;
    this.work = canvas();
    this.packWork = canvas(64, 64);
  }
  // Other fields of view load when first needed; until then the nearest loaded
  // frame stands in, and onLayer asks for a new frame.
  // Слой оружия: FOV — ближайший шире, сдвиг — forwardStep. Пока файл грузится — уже
  // загруженный слой того же сдвига (или 90 и 0, они загружены сразу). null — в игре
  // оружия при этом сдвиге не видно.
  weaponLayer(kind, fov, forward) {
    const step = frameFor(WEAPONS, fov), fwd = forwardStep(forward);
    const at = (WEAPON_META[kind][step] || {})[fwd];
    if (!at) return null;
    const name = `${kind}-${step}-f${fwd}`;
    if (this.images.has(name)) return { step, fwd, at, img: this.images.get(name) };
    if (!this.pending.has(name)) {
      // Не загрузился — забыть запрос: следующая отрисовка попросит снова.
      this.pending.set(name, image(weaponFile(name)).then((img) => { this.images.set(name, img); if (this.onLayer) this.onLayer(); }).catch(() => this.pending.delete(name)));
    }
    const loaded = WEAPONS.find((other) => other >= fov && this.images.has(`${kind}-${other}-f${fwd}`));
    if (loaded) return { step: loaded, fwd, at: WEAPON_META[kind][loaded][fwd], img: this.images.get(`${kind}-${loaded}-f${fwd}`) };
    const base = `${kind}-90-f0`;
    return this.images.has(base) ? { step: 90, fwd: 0, at: WEAPON_META[kind][90][0], img: this.images.get(base) } : null;
  }
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
  // A frame taken at field of view `from`, shown at `to`: enlarged around the
  // centre, then moved by `offset` (weaponOffset).
  place(ctx, img, at, from, to, offset = { scale: 1, x: 0, y: 0 }) {
    const k = Math.min(4, tan(from) / tan(to)) * offset.scale;
    ctx.drawImage(img, CX + (at[0] - CX) * k + offset.x, CY + (at[1] - CY) * k + offset.y, img.width * k, img.height * k);
  }
  // Adds colour × mask (one part of it) to the frame, as the game does.
  add(ctx, mask, colour, box = [0, 0, mask.width, mask.height], at = [0, 0], work = this.work) {
    const w = box[2] - box[0], h = box[3] - box[1];
    const c = work.getContext('2d');
    c.globalCompositeOperation = 'copy'; c.drawImage(mask, box[0], box[1], w, h, 0, 0, w, h);
    if (colour) { c.globalCompositeOperation = 'multiply'; c.fillStyle = css(colour); c.fillRect(0, 0, w, h); }
    ctx.save(); ctx.globalCompositeOperation = 'lighter'; ctx.drawImage(work, 0, 0, w, h, at[0] + box[0], at[1] + box[1], w, h); ctx.restore();
  }
  render(target, value, section = 'personalization', assets = {}) {
    if (!target) return;
    // The HUD is blended pixel by pixel (getImageData). A canvas kept on the
    // GPU would be read back for every element, which made sliders lag;
    // willReadFrequently keeps this one in memory (the first getContext
    // call decides).
    const ctx = target.getContext('2d', { willReadFrequently: true }), s = Object.assign(defaults(), value.extra || {});
    ctx.save();
    ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = 'high';
    ctx.globalAlpha = 1; ctx.globalCompositeOperation = 'source-over';
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, FRAME_WIDTH, FRAME_HEIGHT);

    const world = worldFov(s), scene = this.layer('scene', SCENES, world);
    this.place(ctx, scene.img, [0, 0], scene.step, world);

    // bxt_remove_viewmodel draws no weapon at all.
    if (!s.removeViewmodel) {
      const vm = weaponFov(s), semi = !!s.viewmodelSemitransparent;
      const weapon = this.weaponLayer(semi ? 'semi' : 'weapon', vm, s.viewmodelForward);
      if (weapon) {
        const offset = weaponOffset(s, vm, weapon.fwd);
        if (semi) { ctx.globalCompositeOperation = 'lighter'; this.place(ctx, weapon.img, weapon.at, weapon.step, vm, offset); ctx.globalCompositeOperation = 'source-over'; }
        else this.place(ctx, weapon.img, weapon.at, weapon.step, vm, offset);
      }
    }
    ctx.restore();

    const colour = hudColor(value);
    if (s.packHud === 'ag-hud') this.drawPackHud(ctx, colour);
    else if (colour) this.add(ctx, this.images.get('hud'), colour);
    else this.add(ctx, this.images.get('hud-stock'), null);

    const panels = this.images.get('panels'), tint = colour || BXT_STOCK;
    ['timer', 'speedometer', 'jumpspeed'].forEach((key) => { if (s[key]) this.add(ctx, panels, tint, META.panels[key], panelShift(s, key)); });
    if (s.jumpdistance) this.add(ctx, panels, tint, META.panels.jumpspeed, [0, -NUMBER_HEIGHT]);
    if (s.viewangles) this.drawAngles(ctx, s);

    if (value.crossEnabled) this.drawCrosshair(ctx, value);
    // The weapon's crosshair stays unless the BXT crosshair or the
    // hideCrosshair flag (crosshair 0) hides it.
    if (!s.hideCrosshair && !(value.crossEnabled && value.crossHideDefault)) {
      const pack = crosshairLayer(s, PACKS);
      if (pack) {
        const [x, y, w, h] = pack.box;
        ctx.drawImage(this.images.get(pack.image), x, y, w, h, CX - w / 2, CY - h / 2, w, h);
      } else ctx.drawImage(this.images.get('crosshair'), META.crosshair[0], META.crosshair[1]);
    }
  }
  // The "Angles:" line as it is, then on each value line its label and the
  // number put together from the frame's glyphs.
  drawAngles(ctx, s) {
    const img = this.images.get('viewangles'), text = META.anglesText, shift = panelShift(s, 'viewangles');
    const left = META.viewangles[0] + shift[0], top = META.viewangles[1] + shift[1];
    const digits = Math.max(0, Math.min(16, Math.trunc(number(s.precision, 6))));
    const first = text.lines[0].top;
    ctx.drawImage(img, 0, 0, img.width, first, left, top, img.width, first);
    for (const line of text.lines) {
      ctx.drawImage(img, 0, line.top, line.number, text.height, left, top + line.top, line.number, text.height);
      let x = line.number;
      for (const ch of fixed(line.value, digits)) {
        const [sy, sx] = text.glyphs[ch], w = ch === '.' ? text.point : text.digit;
        ctx.drawImage(img, sx, sy, w, text.height, left + x, top + line.top, w, text.height);
        x += w;
      }
    }
  }
  drawPackHud(ctx, colour) {
    const tint = colour || { r: 100, g: 63, b: 0 };
    for (const item of hudCommands(PACKS.hud, FRAME_WIDTH, FRAME_HEIGHT)) {
      if (item.fill) {
        ctx.save(); ctx.globalCompositeOperation = 'lighter'; ctx.fillStyle = css(tint);
        ctx.fillRect(item.at[0], item.at[1], item.fill[0], item.fill[1]); ctx.restore();
      } else {
        const [x, y, w, h] = item.box;
        this.add(ctx, this.images.get('hud-ag'), tint, [x, y, x + w, y + h], [item.at[0] - x, item.at[1] - y], this.packWork);
      }
    }
  }
  drawCrosshair(ctx, v) {
    const alpha = number(v.crossAlpha, 255) / 255;
    if (!alpha) return;
    const colour = v.hudEnabled && v.hudAffectsCrosshair ? v.hudColor : v.crossColor;
    ctx.save(); ctx.globalAlpha = alpha;
    crosshairPixels(v, CX, CY, colour || { r: 0, g: 255, b: 0 }).forEach((p) => { ctx.fillStyle = css(p.color); ctx.fillRect(p.x, p.y, p.w, p.h); });
    ctx.restore();
  }
}
