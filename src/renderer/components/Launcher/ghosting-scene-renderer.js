// Превью разделов «Призраки и запись» (призрак на пляже) и «Эффекты и спреи»
// (спрей на стене). Расчёты — в src/shared/ghosting-scene-preview.js.
// ghost.webp — программный рендер models/player.mdl из hl2_misc_dir.vpk (поза
// Idle01, кадр 0) без цвета и прозрачности, той же камерой, что у сцены;
// ghost.json — где он в кадре 1920×1080. qa-out/ghost-scene/render-ghost.js
// воспроизводит его из установленной игры.
import { PLAYER_HEIGHT } from './ghost-mesh-sprite';
const scene = require('../../../shared/ghosting-scene-preview');
const { defaults } = require('../../../shared/ghosting-settings');
const SPRITE = require('@/assets/launcher/ghosting-preview/scene/ghost.json');
const FILES = {
  beach: require('@/assets/launcher/ghosting-preview/scene/beach.webp'),
  wall: require('@/assets/launcher/ghosting-preview/scene/wall.webp'),
  ghost: require('@/assets/launcher/ghosting-preview/scene/ghost.webp'),
  spray: require('@/assets/launcher/ghosting-preview/scene/spray-default.webp'),
};
const rgb = (c, a = 1) => `rgba(${c.r},${c.g},${c.b},${a})`;
function canvas(w, h) { const c = document.createElement('canvas'); c.width = Math.max(1, Math.round(w)); c.height = Math.max(1, Math.round(h)); return c; }
function image(src) { const i = new Image(); i.src = src; return i.decode().then(() => i); }

let loading;
export function loadScenes() {
  if (!loading) {
    const names = Object.keys(FILES);
    loading = Promise.all(names.map((name) => image(FILES[name]))).then((list) => new SceneRenderer(Object.fromEntries(names.map((name, i) => [name, list[i]]))));
    loading.catch(() => { loading = null; });
  }
  return loading;
}

export class SceneRenderer {
  constructor(images) {
    this.images = images;
    this.backdrop = null;
    this.tints = new Map();
    this.sprays = new Map();
  }
  render(target, value, kind, assets = {}) {
    const ctx = target.getContext('2d', { willReadFrequently: true });
    ctx.save();
    ctx.globalAlpha = 1; ctx.globalCompositeOperation = 'source-over'; ctx.filter = 'none';
    ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = 'high';
    const s = Object.assign(defaults(), value.extra || {});
    // Для подписи под кадром: каким квадратом и какого размера спрей лёг на стену.
    const info = kind === 'ghost' ? this.drawGhost(ctx, s, value, assets.ghostModel) : this.drawSpray(ctx, s, assets.spray);
    ctx.restore();
    return info;
  }
  // Тот же участок пляжа, что и у призрака, размытый и затемнённый. Готовится
  // один раз на размер кадра: размытие на каждый сдвиг ползунка дорого.
  backdropFor(width, height, crop) {
    if (this.backdrop && this.backdrop.width === width && this.backdrop.height === height) return this.backdrop;
    const c = canvas(width, height), bc = c.getContext('2d'), img = this.images.beach, k = img.width / scene.FRAME.width;
    const blur = scene.BACKDROP.blur * height / 900, pad = blur * 3;
    bc.filter = `blur(${blur}px)`;
    // Поля с запасом, чтобы края не темнели от размытия.
    const s = width / crop.width;
    bc.drawImage(img, (crop.x - pad / s) * k, (crop.y - pad / s) * k, (crop.width + 2 * pad / s) * k, (crop.height + 2 * pad / s) * k, -pad, -pad, width + 2 * pad, height + 2 * pad);
    bc.filter = 'none';
    bc.fillStyle = `rgba(0,0,0,${scene.BACKDROP.dim})`; bc.fillRect(0, 0, width, height);
    this.backdrop = c;
    return c;
  }
  // Кадр любой ширины: высота участка постоянна, призрак всегда по центру.
  // model — своя модель (ghost-mesh-sprite.js): ноги там же, где у готового призрака, масштаб —
  // по росту модели игрока (PLAYER_HEIGHT), так что высокая модель и в кадре выше.
  drawGhost(ctx, s, value, model) {
    const width = ctx.canvas.width, height = ctx.canvas.height, crop = scene.ghostView(width / height), k = height / crop.height;
    const at = (x, y) => [(x - crop.x) * k, (y - crop.y) * k];
    ctx.drawImage(this.backdropFor(width, height, crop), 0, 0);
    this.drawTrail(ctx, scene.trailPoints(s).map((p) => { const [x, y] = at(p.x, p.y), [ex, ey] = at(p.ex, p.ey); return { x, y, ex, ey, alpha: p.alpha }; }), s.gh_trail_color);
    // kRenderTransColor: цвет умножает модель, gh_opacity — её альфа.
    const alpha = scene.ghostAlpha(s.gh_opacity, s.gh_opacity_range) / 255;
    if (alpha > 0 && model) {
      const unit = SPRITE.height / PLAYER_HEIGHT * k / model.ppu;
      const [fx, fy] = at(SPRITE.x + SPRITE.width / 2, SPRITE.y + SPRITE.height);
      ctx.globalAlpha = alpha;
      ctx.drawImage(this.tinted(s.gh_ghost_color, model.canvas), fx - model.originX * unit, fy - model.groundY * unit, model.canvas.width * unit, model.canvas.height * unit);
      ctx.globalAlpha = 1;
    } else if (alpha > 0) {
      const [x, y] = at(SPRITE.x, SPRITE.y);
      ctx.globalAlpha = alpha;
      ctx.drawImage(this.tinted(s.gh_ghost_color), x, y, SPRITE.width * k, SPRITE.height * k);
      ctx.globalAlpha = 1;
    }
    const name = scene.ghostName(s, value.hudEnabled ? value.hudColor : null);
    if (name) {
      const [x, y] = at(name.x, name.y), size = name.size * k;
      ctx.font = `700 ${size}px Verdana, "DejaVu Sans", sans-serif`; ctx.textAlign = 'center'; ctx.textBaseline = 'bottom';
      ctx.fillStyle = `rgba(0,0,0,${name.alpha * 180 / 255 / 255})`; ctx.fillText(name.text, x + k, y + k);
      ctx.fillStyle = rgb(name.color, name.alpha / 255); ctx.fillText(name.text, x, y);
    }
  }
  tinted(color, img = this.images.ghost) {
    // Своя модель меняется — её окраски отдельно (по картинке), готовый призрак — по цвету.
    if (img !== this.tintSource) { this.tints.clear(); this.tintSource = img; }
    const key = rgb(color);
    if (this.tints.has(key)) return this.tints.get(key);
    const c = canvas(img.width, img.height), tc = c.getContext('2d');
    tc.drawImage(img, 0, 0);
    tc.globalCompositeOperation = 'multiply'; tc.fillStyle = key; tc.fillRect(0, 0, c.width, c.height);
    tc.globalCompositeOperation = 'destination-in'; tc.drawImage(img, 0, 0);
    // У своей модели канвас большой: хранится одна окраска, у готового призрака — до 16.
    if (this.tints.size > (img === this.images.ghost ? 16 : 0)) this.tints.clear();
    this.tints.set(key, c);
    return c;
  }
  // Лента laser.vtf, сложением (rendermode 5): поперёк — яркость текстуры от
  // середины к краю, вдоль — renderamt × оставшаяся жизнь точки.
  drawTrail(ctx, points, color) {
    if (points.length < 2) return;
    const width = ctx.canvas.width, height = ctx.canvas.height, profile = scene.LASER_HALF;
    ctx.globalCompositeOperation = 'lighter';
    for (let i = 0; i + 1 < points.length; i++) {
      const a = points[i], b = points[i + 1];
      const xs = [a.x, b.x, a.ex, b.ex], ys = [a.y, b.y, a.ey, b.ey];
      if (Math.max(...xs) < 0 || Math.min(...xs) > width || Math.max(...ys) < 0 || Math.min(...ys) > height) continue;
      const alpha = (a.alpha + b.alpha) / 2;
      const gradient = ctx.createLinearGradient((a.x + b.x) / 2, (a.y + b.y) / 2, (a.ex + b.ex) / 2, (a.ey + b.ey) / 2);
      profile.forEach((v, j) => gradient.addColorStop(j / (profile.length - 1), rgb(color, v / 255 * alpha)));
      ctx.fillStyle = gradient;
      ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.lineTo(b.ex, b.ey); ctx.lineTo(a.ex, a.ey); ctx.closePath(); ctx.fill();
    }
    ctx.globalCompositeOperation = 'source-over';
  }
  // Спрей там, куда смотрит игрок, — в середине кадра. Игра сжимает картинку
  // в квадрат (ресемплинг ящиком) и кладёт его декалью на стену.
  drawSpray(ctx, s, picked) {
    const width = ctx.canvas.width, height = ctx.canvas.height;
    ctx.drawImage(this.images.wall, 0, 0, width, height);
    if (!s.gh_sprays) return;
    const img = picked || this.images.spray;
    const layout = scene.sprayLayout(s.gh_spray_scale, img.naturalWidth || img.width, img.naturalHeight || img.height, width);
    ctx.drawImage(this.square(img, layout.texels), (width - layout.pixels) / 2, (height - layout.pixels) / 2, layout.pixels, layout.pixels);
    return { units: layout.units, texels: layout.texels };
  }
  square(img, texels) {
    const cached = this.sprays.get(img);
    if (cached && cached.width === texels) return cached;
    const c = canvas(texels, texels), sc = c.getContext('2d');
    sc.imageSmoothingEnabled = true; sc.imageSmoothingQuality = 'high';
    sc.drawImage(img, 0, 0, texels, texels);
    // Декаль LightmappedGeneric берёт освещение стены: в тени фона она темнее.
    sc.globalCompositeOperation = 'source-atop'; sc.fillStyle = 'rgba(0,0,0,0.12)'; sc.fillRect(0, 0, texels, texels);
    if (this.sprays.size > 4) this.sprays.clear();
    this.sprays.set(img, c);
    return c;
  }
}
