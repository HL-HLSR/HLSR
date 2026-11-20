"use strict";

// Размер панели у разделителя (PaneSplitter.vue): пределы, шаг клавиш и запоминание.

const PREFIX = "hlsr.pane.";
const STEP = 16;

// Окно меньше двух минимумов (max < min) — побеждает min, как у CSS clamp().
function clampSize(size, min, max) {
  return Math.round(Math.max(min, Math.min(max, size)));
}

// null — размер не запоминали или хранилище недоступно.
function readSize(storage, key) {
  try {
    const value = Number(storage.getItem(PREFIX + key));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch (error) {
    return null;
  }
}

// size null — забыть: панель вернётся к размеру по умолчанию.
function writeSize(storage, key, size) {
  try {
    if (size == null) storage.removeItem(PREFIX + key);
    else storage.setItem(PREFIX + key, String(Math.round(size)));
  } catch (error) {
    // Размер просто не запомнится.
  }
}

// Новый размер по клавише; null — клавиша не меняет размер.
function keySize(size, key, min, max) {
  switch (key) {
    case "ArrowUp":
    case "ArrowLeft":
      return clampSize(size - STEP, min, max);
    case "ArrowDown":
    case "ArrowRight":
      return clampSize(size + STEP, min, max);
    case "Home":
      return min;
    case "End":
      return clampSize(max, min, max);
    default:
      return null;
  }
}

module.exports = { STEP, clampSize, readSize, writeSize, keySize };
