// Своя модель призрака (gh_model) в предпросмотре «Призраки и запись»: треугольники из
// ghost-model.js рисуются серым силуэтом со светом сверху-спереди, а сцена красит его цветом
// призрака так же, как готовый ghost.webp (multiply). Поза — привязки (как в .vvd), без
// анимации и текстур: игра тоже заливает призрака цветом.

// Рост models/player.mdl в единицах игры: столько занимает ghost.webp в кадре.
export const PLAYER_HEIGHT = 72;
// Модель чуть повёрнута к камере, как призрак на готовом кадре.
const YAW = -0.45;
// Направление на свет в осях вида (глубина к камере, вправо, вверх).
const LIGHT = (() => { const v = [0.65, -0.35, 0.68], n = Math.hypot(...v); return v.map((x) => x / n); })();

// mesh: { positions: [x, y, z × 3 на треугольник] }. ppu — пикселей на единицу игры.
// Ответ: { canvas, ppu, originX, groundY } — где в канвасе ось модели и земля (низ модели).
export function meshSprite(mesh, ppu = 4) {
  const p = mesh && mesh.positions;
  if (!p || p.length < 9) return null;
  const cos = Math.cos(YAW), sin = Math.sin(YAW);
  const n = p.length / 3, depth = new Float32Array(n), sx = new Float32Array(n), sy = new Float32Array(n);
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  for (let i = 0; i < n; i += 1) {
    const x = p[i * 3], y = p[i * 3 + 1], z = p[i * 3 + 2];
    // Модель Source смотрит вдоль +X; камера стоит на +X и смотрит на неё: вправо — +Y.
    depth[i] = x * cos - y * sin;
    sx[i] = x * sin + y * cos;
    sy[i] = -z;
    if (sx[i] < minX) minX = sx[i]; if (sx[i] > maxX) maxX = sx[i];
    if (sy[i] < minY) minY = sy[i]; if (sy[i] > maxY) maxY = sy[i];
  }
  // Огромная модель не даёт огромный канвас: масштаб уменьшается до стороны 2048.
  const span = Math.max(maxX - minX, maxY - minY, 1e-3);
  ppu = Math.min(ppu, 2040 / span);
  const pad = 2, width = Math.ceil((maxX - minX) * ppu) + pad * 2, height = Math.ceil((maxY - minY) * ppu) + pad * 2;
  const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
  const ctx = canvas.getContext('2d');
  const px = (i) => (sx[i] - minX) * ppu + pad, py = (i) => (sy[i] - minY) * ppu + pad;
  // Дальние треугольники — первыми (художник).
  const tris = Array.from({ length: n / 3 }, (_, t) => t);
  const far = new Float32Array(n / 3);
  for (let t = 0; t < n / 3; t += 1) far[t] = depth[t * 3] + depth[t * 3 + 1] + depth[t * 3 + 2];
  tris.sort((a, b) => far[a] - far[b]);
  ctx.lineJoin = 'round'; ctx.lineWidth = 0.6;
  for (const t of tris) {
    const a = t * 3, b = a + 1, c = a + 2;
    // Нормаль в осях вида; свет с двух сторон: порядок обхода у моделей бывает разный.
    const ux = depth[b] - depth[a], uy = sx[b] - sx[a], uz = -(sy[b] - sy[a]);
    const vx = depth[c] - depth[a], vy = sx[c] - sx[a], vz = -(sy[c] - sy[a]);
    const nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx, len = Math.hypot(nx, ny, nz) || 1;
    const lit = Math.abs((nx * LIGHT[0] + ny * LIGHT[1] + nz * LIGHT[2]) / len);
    const v = Math.round(255 * (0.28 + 0.62 * lit));
    ctx.fillStyle = ctx.strokeStyle = `rgb(${v},${v},${v})`;
    ctx.beginPath(); ctx.moveTo(px(a), py(a)); ctx.lineTo(px(b), py(b)); ctx.lineTo(px(c), py(c)); ctx.closePath();
    ctx.fill(); ctx.stroke(); // обводка закрывает щели сглаживания между соседними треугольниками
  }
  return { canvas, ppu, originX: -minX * ppu + pad, groundY: (maxY - minY) * ppu + pad };
}
