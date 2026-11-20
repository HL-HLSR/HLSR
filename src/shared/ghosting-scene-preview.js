"use strict";
// Превью разделов «Призраки и запись» и «Эффекты и спреи» в персонализации
// HL2 Ghosting. Все числа — из исходников мода (gh2013, ветка gh2013_main):
// GhostEntity.cpp и gameinterface.cpp (модель, цвет, прозрачность, след),
// GhostHud.cpp (имена), SpriteTrail.cpp (как рисуется след), gh_spray.cpp и
// gh_image.cpp (спрей). Чистые расчёты без DOM: их проверяет
// scripts/ghosting-scene-preview-selftest.js.

// Призрак показан как в кадре игры 1920×1080 с fov_desired 90 (у Source это
// угол для 4:3 той же высоты, фокус — 720 px). Игрок стоит в начале координат
// и смотрит вдоль +x; в Source x — вперёд, y — влево, z — вверх.
const FRAME = { width: 1920, height: 1080 };
const FOCAL = FRAME.height * 2 / 3;
const EYE_HEIGHT = 64;
// Призрак стоит в 220 юнитах: дальше gh_opacity_range по умолчанию (200),
// поэтому стандартная непрозрачность видна как есть. Бежит к игроку и вправо.
const GHOST = { origin: [212, -60, 0], yaw: 215 };
// Превью — вертикальный участок кадра (портрет, ширину задаёт окно превью):
// призрак с именем строго по центру, сверху и снизу по 13 % запаса.
// x — середина модели в кадре (ghost.json), y — середина от верха имени до ног.
const GHOST_VIEW = { x: 1171.5, y: 627, height: 380 };
// Фон сцены призрака: размытие в пикселях кадра превью и затемнение чёрным.
const BACKDROP = { blur: 10, dim: 0.2 };

// env_spritetrail из GhostEntity::CreateTrail: rendermode 5 (сложение),
// renderamt 75, ширина 9.5 у призрака и 1.05 в хвосте, materials/sprites/laser.
const TRAIL = { startWidth: 9.5, endWidth: 1.05, brightness: 75 };
// Яркость laser.vtf поперёк ленты, от середины к краю (столбцы текстуры
// 32×64, усреднены). Нижняя половина ленты уходит под землю: след крепится к
// origin призрака, то есть к его ногам.
const LASER_HALF = [246, 69, 66, 62, 56, 49, 40, 32, 24, 15, 5, 0, 0, 0, 0, 0];
// Скорость и путь призрака за последние секунды: бежит издалека почти на
// игрока и у самой камеры сворачивает вправо, поэтому след уходит к горизонту
// левее призрака и виден в узком кадре целиком. shortcut: путь придуман, а не
// взят из настоящего забега; заменить записью .run, если понадобится.
const TRAIL_SPEED = 300;
const TRAIL_FAR_YAW = 172;
const TRAIL_STEP = 1 / 30;

// Имена: GhostHud.cpp, CHudGhostNames. Verdana 700, высота gh_ghost_names_size
// (6 единиц HudLayout 640×480), прозрачность gh_ghost_names_alpha 180, тень
// (0,0,0, alpha·180/255) со сдвигом 1 px, над глазами (+12 юнитов).
const NAMES = { size: 6, alpha: 180, fadeStart: 250, raise: 12, fallback: { r: 255, g: 220, b: 70 } };

// Спрей: картинка сжимается в квадрат со стороной — степень двойки от 8 до
// 256 по длинной стороне, на стене это сторона × gh_spray_scale юнитов.
// Стена на фоне превью — 192 юнита в ширину. shortcut: ширина подобрана на
// глаз (игрок в шаге от стены), а не измерена в игре.
const SPRAY = { min: 8, max: 256, wallUnits: 192 };

const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
const number = (value, fallback) => (Number.isFinite(Number(value)) ? Number(value) : fallback);

// Точка мира → пиксели кадра 1920×1080 (z — глубина).
function project(point) {
  const depth = point[0];
  return { x: FRAME.width / 2 - FOCAL * point[1] / depth, y: FRAME.height / 2 - FOCAL * (point[2] - EYE_HEIGHT) / depth, z: depth };
}
// Участок кадра игры, который показывает превью с соотношением сторон aspect.
function ghostView(aspect) {
  const width = GHOST_VIEW.height * aspect;
  return { x: GHOST_VIEW.x - width / 2, y: GHOST_VIEW.y - GHOST_VIEW.height / 2, width, height: GHOST_VIEW.height };
}

const ghostDistance = () => Math.hypot(GHOST.origin[0], GHOST.origin[1], GHOST.origin[2]);

// gameinterface.cpp: ближе gh_opacity_range призрак становится прозрачнее
// пропорционально расстоянию до игрока.
function ghostAlpha(opacity, range) {
  const base = clamp(Math.trunc(number(opacity, 75)), 0, 255);
  const limit = number(range, 200), distance = ghostDistance();
  if (limit > 0 && distance <= limit) return clamp(Math.trunc(distance * (base / limit)), 0, 255);
  return base;
}

// Имя над призраком или null, если игра его не рисует. hudColor — цвет HUD
// лаунчера (gh_hud_color) или null, если он выключен.
function ghostName(settings, hudColor) {
  const name = String(settings.gh_name || "");
  if (!settings.gh_ghost_names || !name) return null;
  // Расстояние считается от глаз игрока до origin призрака.
  const eye = [0, 0, EYE_HEIGHT], o = GHOST.origin;
  const distance = Math.hypot(o[0] - eye[0], o[1] - eye[1], o[2] - eye[2]);
  const max = number(settings.gh_ghost_names_dist, 2000);
  if (max > 0 && distance > max) return null;
  let fraction = 1;
  if (max > 0) {
    const from = NAMES.fadeStart;
    if (distance > from) fraction = 1 - (distance - from) / Math.max(1, max - from);
  }
  const alpha = Math.trunc(NAMES.alpha * clamp(fraction, 0, 1) + 0.5);
  if (alpha <= 0) return null;
  const color = settings.gh_ghost_names_own_color ? settings.gh_ghost_color : (hudColor || NAMES.fallback);
  const anchor = project([o[0], o[1], o[2] + EYE_HEIGHT + NAMES.raise]);
  // GetProportionalScaledValueEx: единицы 480 строк → пиксели экрана.
  return { text: name.slice(0, 63), alpha, color, size: Math.max(1, Math.trunc(NAMES.size * FRAME.height / 480)), x: anchor.x, y: anchor.y };
}

// Направление движения (yaw, градусы) за t секунд до кадра.
const heading = (t) => TRAIL_FAR_YAW + (GHOST.yaw - TRAIL_FAR_YAW) * Math.exp(-t / 0.12) - 6 * Math.sin(2.2 * t) * Math.exp(-t / 2.5);

// Точки следа в пикселях кадра, от призрака к хвосту: x, y — середина ленты,
// ex, ey — её видимый край, alpha — доля яркости (renderamt × оставшаяся
// жизнь точки). Лента повёрнута к камере, как в CBeamSegDraw::ComputeNormal,
// её половина ниже земли не видна.
function trailPoints(settings) {
  const life = number(settings.gh_trail_length, 5);
  if (!settings.gh_draw_trails || !(life > 0)) return [];
  const path = [];
  let x = GHOST.origin[0], y = GHOST.origin[1];
  for (let t = 0; t <= life + 1e-9; t += TRAIL_STEP) {
    path.push([x, y, GHOST.origin[2], t]);
    const a = heading(t) * Math.PI / 180;
    x -= Math.cos(a) * TRAIL_SPEED * TRAIL_STEP;
    y -= Math.sin(a) * TRAIL_SPEED * TRAIL_STEP;
  }
  const points = [];
  for (let i = 0; i < path.length; i++) {
    const p = path[i], a = path[Math.max(0, i - 1)], b = path[Math.min(path.length - 1, i + 1)];
    const tangent = [a[0] - b[0], a[1] - b[1], 0];
    const view = [p[0], p[1], p[2] - EYE_HEIGHT];
    const n = [tangent[1] * view[2] - tangent[2] * view[1], tangent[2] * view[0] - tangent[0] * view[2], tangent[0] * view[1] - tangent[1] * view[0]];
    const length = Math.hypot(n[0], n[1], n[2]) || 1, side = n[2] < 0 ? -1 : 1;
    const left = 1 - p[3] / life, half = (TRAIL.endWidth + (TRAIL.startWidth - TRAIL.endWidth) * left) / 2 * side / length;
    const c = project(p), e = project([p[0] + n[0] * half, p[1] + n[1] * half, p[2] + n[2] * half]);
    if (Math.hypot(e.x - c.x, e.y - c.y) < 0.05) break;
    points.push({ x: c.x, y: c.y, ex: e.x, ey: e.y, alpha: TRAIL.brightness / 255 * left });
  }
  return points;
}

// Как игра делает спрей из картинки w×h и какого он размера на стене.
function sprayLayout(scale, width, height, wallWidth) {
  const longest = Math.max(1, Math.min(SPRAY.max, Math.max(width || 0, height || 0)));
  let texels = SPRAY.min;
  while (texels * 2 <= longest && texels * 2 <= SPRAY.max) texels *= 2;
  const units = texels * clamp(number(scale, 0.25), 0.05, 2);
  return { texels, units, pixels: units * wallWidth / SPRAY.wallUnits };
}

module.exports = { FRAME, FOCAL, EYE_HEIGHT, GHOST, GHOST_VIEW, BACKDROP, TRAIL, LASER_HALF, NAMES, SPRAY, project, ghostView, ghostDistance, ghostAlpha, ghostName, trailPoints, sprayLayout };
