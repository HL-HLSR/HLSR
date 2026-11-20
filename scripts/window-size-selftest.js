"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");

const { computeWindowSize, GRID_STEP, MIN_SIZE } = require("../src/main/window-size");

test("Окно: обе стороны кратны сетке интерфейса и не выходят за рабочую область", () => {
  const areas = [
    [1920, 1040], // FullHD с панелью задач
    [2048, 1108], // 1440p при 125 %
    [2048, 1100], // 1440p при 125 %, плавающая панель KDE
    [2560, 1400],
    [1366, 728],
    [3840, 2120],
    [1080, 1880], // вертикальный экран уже минимальной ширины
    [1150, 600], // рабочая область меньше минимального окна
  ];

  for (const [width, height] of areas) {
    const size = computeWindowSize({ width, height });
    const label = `${width}x${height} -> ${size.width}x${size.height}`;

    assert.equal(size.width % GRID_STEP, 0, label);
    assert.equal(size.height % GRID_STEP, 0, label);
    assert.ok(size.width <= width && size.height <= height, label);
  }
});

test("Окно: привычные экраны дают прежние размеры с точностью до шага сетки", () => {
  assert.deepEqual(computeWindowSize({ width: 1920, height: 1040 }), { width: 1200, height: 676 });
  assert.deepEqual(computeWindowSize({ width: 2048, height: 1108 }), { width: 1280, height: 720 });
  assert.deepEqual(computeWindowSize({ width: 2560, height: 1400 }), { width: 1600, height: 900 });
  assert.deepEqual(computeWindowSize({ width: 1366, height: 728 }), MIN_SIZE);
});

test("Окно: масштаб интерфейса меняет окно и zoomFactor вместе", () => {
  const { scaleLayout, normalizeUiScale } = require("../src/main/window-size");
  const area = { width: 2560, height: 1400 };
  const base = computeWindowSize(area);

  // 100 % — размер как до 0.4.3, без увеличения страницы
  assert.deepEqual(scaleLayout(base, area, 1), Object.assign({}, base, { zoomFactor: 1, uiScale: 1 }));
  const small = scaleLayout(base, area, 0.75);
  assert.equal(small.uiScale, 0.75);
  assert.equal(small.width, 1200);
  assert.equal(small.zoomFactor, 0.75);
  // CSS-пиксели раскладки те же
  assert.ok(Math.abs(small.width / small.zoomFactor - base.width) < 1);

  // 1440p: 150 % помещается (1600x900 → 2400x1350)
  assert.equal(scaleLayout(base, area, 1.5).uiScale, 1.5);

  // FullHD: 150 % помещается (1200x676 → 1800x1014)
  const hd = { width: 1920, height: 1040 };
  assert.equal(scaleLayout(computeWindowSize(hd), hd, 1.5).uiScale, 1.5);

  // 1366x768: 125 % не помещается целиком, окно увеличивается сколько влезает
  const small768 = { width: 1366, height: 728 };
  const big = scaleLayout(computeWindowSize(small768), small768, 1.25);
  assert.ok(big.uiScale > 1 && big.uiScale < 1.25, String(big.uiScale));
  assert.ok(big.width <= small768.width && big.height <= small768.height);

  // Full HD: раскладка как 2K (1600×900 CSS), окно уменьшает её целиком
  for (const [screen, zoom] of [[hd, 0.75], [{ width: 1536, height: 824 }, 0.72]]) {
    const fhd = scaleLayout(computeWindowSize(screen), screen, 1);
    assert.ok(Math.abs(fhd.zoomFactor - zoom) < 0.01, String(fhd.zoomFactor));
    assert.ok(Math.abs(fhd.width / fhd.zoomFactor - 1600) < 2 && Math.abs(fhd.height / fhd.zoomFactor - 900) < 2);
    assert.equal(fhd.uiScale, 1);
  }
  // Масштаб из настроек умножается: Full HD 125 % — окно 1500×845, раскладка всё так же 1600×900
  const fhdBig = scaleLayout(computeWindowSize(hd), hd, 1.25);
  assert.ok(Math.abs(fhdBig.width / fhdBig.zoomFactor - 1600) < 2, String(fhdBig.zoomFactor));

  assert.equal(normalizeUiScale("1.25"), 1.25);
  assert.equal(normalizeUiScale(2), 1);
  assert.equal(normalizeUiScale(undefined), 1);
});

test("полный экран: раскладка 1600×900 растягивается на экран целиком", () => {
  const { fillZoom } = require("../src/main/window-size");
  assert.equal(fillZoom({ width: 2560, height: 1440 }), 1.6);
  assert.equal(fillZoom({ width: 1920, height: 1080 }), 1.2);
  assert.equal(fillZoom({ width: 1920, height: 1200 }), 1.2, "16:10 — по ширине, снизу больше места");
  assert.equal(fillZoom({ width: 3440, height: 1440 }), 1.6, "21:9 — по высоте");
  assert.equal(fillZoom({ width: 1366, height: 768 }), 768 / 900, "1366×768 — по высоте");
});
