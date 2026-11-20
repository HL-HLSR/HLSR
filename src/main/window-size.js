"use strict";

const BASE_SIZE = { width: 1600, height: 900 };

const MIN_SIZE = { width: 1152, height: 648 };

const TARGET_WIDTH_FRACTION = 0.625;
const TARGET_HEIGHT_FRACTION = 0.65;

const WORK_AREA_MARGIN = 32;

// Интерфейс разложен по сетке 4 px от краёв окна (renderer/assets/css/
// launcher.css). При ширине 1271 всё, что прижато к правому краю, стояло бы
// на дробной координате, а на экране с масштабом 125 % 4 px — это ровно 5
// физических пикселей. Поэтому обе стороны окна кратны 4.
const GRID_STEP = 4;

function normalizeWorkArea(workArea) {
  const width = workArea && Number.isFinite(workArea.width) ? Math.floor(workArea.width) : 0;
  const height = workArea && Number.isFinite(workArea.height) ? Math.floor(workArea.height) : 0;

  if (width < 1 || height < 1) return { width: BASE_SIZE.width, height: BASE_SIZE.height };
  return { width, height };
}

// Ближайшее кратное шагу, но не больше рабочей области
function snapToGrid(value, limit) {
  const snapped = Math.round(value / GRID_STEP) * GRID_STEP;
  return snapped <= limit ? snapped : Math.floor(limit / GRID_STEP) * GRID_STEP;
}

function computeWindowSize(workArea) {
  const area = normalizeWorkArea(workArea);

  const desired = Math.min(
    (area.width * TARGET_WIDTH_FRACTION) / BASE_SIZE.width,
    (area.height * TARGET_HEIGHT_FRACTION) / BASE_SIZE.height
  );

  const fitting = Math.min(
    (area.width - WORK_AREA_MARGIN) / BASE_SIZE.width,
    (area.height - WORK_AREA_MARGIN) / BASE_SIZE.height
  );

  const scale = Math.min(1, desired, fitting);

  let width = Math.max(MIN_SIZE.width, Math.round(BASE_SIZE.width * scale));
  let height = Math.max(MIN_SIZE.height, Math.round(BASE_SIZE.height * scale));

  width = Math.min(width, area.width);
  height = Math.min(height, area.height);

  return { width: snapToGrid(width, area.width), height: snapToGrid(height, area.height) };
}

// Масштаб интерфейса из настроек («Интерфейс»): окно и zoomFactor растут
// вместе, поэтому раскладка внутри получает те же CSS-пиксели. Окно,
// которое не помещается в рабочую область, увеличивается сколько влезает.
// size — размер из computeWindowSize.
const UI_SCALES = [0.75, 1, 1.25, 1.5];
const DEFAULT_UI_SCALE = 1;

function normalizeUiScale(value) {
  const scale = Number(value);
  return UI_SCALES.includes(scale) ? scale : DEFAULT_UI_SCALE;
}

// Окно меньше базового (Full HD: 1200×676, 1152×648) раскладывается как 2K —
// в BASE_SIZE CSS-пикселей — и уменьшается целиком: всё как на 1600×900, только мельче
// (решение владельца 13.11). Масштаб из настроек умножается на это уменьшение.
function layoutZoom(size) {
  return Math.min(1, size.width / BASE_SIZE.width, size.height / BASE_SIZE.height);
}

// Полный экран (кнопка в шапке): раскладка BASE_SIZE растягивается на весь
// экран display.bounds, как окно меньше 2K уменьшается в layoutZoom. Масштаб
// интерфейса здесь не участвует: окно больше экрана не станет.
function fillZoom(area) {
  const { width, height } = normalizeWorkArea(area);
  return Math.min(width / BASE_SIZE.width, height / BASE_SIZE.height);
}

function scaleLayout(size, workArea, uiScale) {
  const area = normalizeWorkArea(workArea);
  const wanted = normalizeUiScale(uiScale);
  const fit = Math.min(area.width / size.width, area.height / size.height);
  const scale = wanted > 1 ? Math.max(1, Math.min(wanted, fit)) : wanted;
  return {
    width: Math.min(area.width, Math.round(size.width * scale)),
    height: Math.min(area.height, Math.round(size.height * scale)),
    zoomFactor: scale * layoutZoom(size),
    uiScale: scale,
  };
}

module.exports = {
  BASE_SIZE,
  MIN_SIZE,
  TARGET_WIDTH_FRACTION,
  TARGET_HEIGHT_FRACTION,
  WORK_AREA_MARGIN,
  GRID_STEP,
  computeWindowSize,
  UI_SCALES,
  DEFAULT_UI_SCALE,
  normalizeUiScale,
  layoutZoom,
  fillZoom,
  scaleLayout,
};
