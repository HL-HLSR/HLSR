// The Bunnymod XT crosshair (bxt_cross_*) as the pixels the game fills.
// BXT draws it with plain OpenGL lines (hud_custom.cpp, DrawCrosshair): first
// the outline as a frame of lines around every bar, then the bars, the circle
// and the dot. Without anti-aliasing a line of width w covers the pixels whose
// centres lie within w/2 of it, so every primitive here is a list of whole
// pixel rectangles. Checked against frames of the game: off by a few pixels at
// most, on the circle. Each primitive is blended on its own, as in the game.
//
// Returns [{ x, y, w, h, color }] in drawing order; color is { r, g, b }.

const BLACK = { r: 0, g: 0, b: 0 };

// Pixels a line covers along its length. Measured against the game: a
// horizontal line ends before its last point, a vertical one starts after its
// first.
function span(from, to, vertical) {
  const round = vertical ? (v) => Math.floor(v + 0.5) : (v) => Math.ceil(v - 0.5);
  const a = round(Math.min(from, to)), b = round(Math.max(from, to));
  return [a, b - a];
}

// A wide line is as wide as the rounded width, at least one pixel. On a pixel
// edge a horizontal line takes the row below it, a vertical one the column to
// its left (as the game draws them).
function across(centre, width, vertical) {
  const w = Math.max(1, Math.round(width));
  return [vertical ? Math.ceil(centre - w / 2 - 0.5) : Math.floor(centre - w / 2 + 0.5), w];
}

function line(out, x0, y0, x1, y1, width, color) {
  if (y0 === y1) {
    const [x, w] = span(x0, x1, false), [y, h] = across(y0, width, false);
    if (w > 0) out.push({ x, y, w, h, color });
  } else if (x0 === x1) {
    const [y, h] = span(y0, y1, true), [x, w] = across(x0, width, true);
    if (h > 0) out.push({ x, y, w, h, color });
  }
}

// A one pixel line strip through the points, closed: one pixel per step along
// the major axis, the pixel the line passes through.
function strip(out, points, color) {
  const seen = new Set();
  const put = (x, y) => {
    const key = x + ',' + y;
    if (seen.has(key)) return;
    seen.add(key);
    out.push({ x, y, w: 1, h: 1, color });
  };
  for (let i = 0; i < points.length; i++) {
    const [ax, ay] = points[i], [bx, by] = points[(i + 1) % points.length];
    const dx = bx - ax, dy = by - ay;
    if (Math.abs(dx) >= Math.abs(dy)) {
      const [x0, n] = span(ax, bx, false);
      for (let x = x0; x < x0 + n; x++) put(x, Math.floor(ay + dy * ((x + 0.5 - ax) / (dx || 1))));
    } else {
      const [y0, n] = span(ay, by, true);
      for (let y = y0; y < y0 + n; y++) put(Math.floor(ax + dx * ((y + 0.5 - ay) / (dy || 1))), y);
    }
  }
}

function circlePoints(cx, cy, radius) {
  const count = Math.ceil(Math.PI / Math.acos((radius - 0.1) / radius));
  const points = [];
  for (let i = 0; i < count; i++) {
    const angle = Math.PI * 2 * i / count;
    points.push([cx + radius * Math.cos(angle), cy + radius * Math.sin(angle)]);
  }
  return points;
}

export function crosshairPixels(v, cx, cy, color) {
  const out = [];
  const size = Number(v.crossSize) || 0, gap = Number(v.crossGap) || 0, thickness = Number(v.crossThickness) || 0;
  const lines = { top: v.crossTopLine, bottom: v.crossBottomLine, left: v.crossLeftLine, right: v.crossRightLine };
  const outline = v.crossOutline ? Number(v.crossOutlineThickness) || 0 : 0;
  const dot = Number(v.crossDotSize) || 0;

  if (outline) {
    const ht = thickness / 2, hw = outline / 2, o = ht + hw;
    const frame = (segments) => segments.forEach((s) => line(out, s[0], s[1], s[2], s[3], outline, BLACK));
    if (lines.top) frame([[cx - o, cy - gap - size, cx + o, cy - gap - size], [cx + ht, cy - gap - size + hw, cx + ht, cy - gap - hw], [cx + o, cy - gap, cx - o, cy - gap], [cx - ht, cy - gap - hw, cx - ht, cy - gap - size + hw]]);
    if (lines.bottom) frame([[cx - o, cy + gap + size, cx + o, cy + gap + size], [cx + ht, cy + gap + size - hw, cx + ht, cy + gap + hw], [cx + o, cy + gap, cx - o, cy + gap], [cx - ht, cy + gap + hw, cx - ht, cy + gap + size - hw]]);
    if (lines.left) frame([[cx - gap - size, cy - o, cx - gap - size, cy + o], [cx - gap - size + hw, cy + ht, cx - gap - hw, cy + ht], [cx - gap, cy + o, cx - gap, cy - o], [cx - gap - hw, cy - ht, cx - gap - size + hw, cy - ht]]);
    if (lines.right) frame([[cx + gap + size, cy - o, cx + gap + size, cy + o], [cx + gap + size - hw, cy + ht, cx + gap + hw, cy + ht], [cx + gap, cy + o, cx + gap, cy - o], [cx + gap + hw, cy - ht, cx + gap + size - hw, cy - ht]]);
    if (dot) {
      const d = dot / 2;
      frame([[cx - d - hw, cy - d, cx + d + hw, cy - d], [cx + d, cy - d + hw, cx + d, cy + d - hw], [cx - d, cy - d + hw, cx - d, cy + d - hw], [cx - d - hw, cy + d, cx + d + hw, cy + d]]);
    }
  }

  if (thickness) {
    if (lines.top) line(out, cx, cy - gap - size, cx, cy - gap, thickness, color);
    if (lines.bottom) line(out, cx, cy + gap + size, cx, cy + gap, thickness, color);
    if (lines.left) line(out, cx - gap - size, cy, cx - gap, cy, thickness, color);
    if (lines.right) line(out, cx + gap + size, cy, cx + gap, cy, thickness, color);
  }

  if (v.crossCircleRadius > 0) strip(out, circlePoints(cx, cy, Number(v.crossCircleRadius)), color);

  if (dot) {
    const d = dot / 2, [x, w] = span(cx - d, cx + d, false), [y, h] = span(cy - d, cy + d, true);
    if (w > 0 && h > 0) out.push({ x, y, w, h, color: v.crossDotColor || color });
  }
  return out;
}
