"use strict";

// Платформа, под которую идёт сборка. Обычно это платформа машины, но
// Windows-установщик собирается и на Linux (npm run build:win): тогда
// HLSR_TARGET_PLATFORM=win32, и от него, а не от process.platform, зависят
// апдейтер в app.asar, ключ аттестации и Electron, компилирующий байткод.
const ENV = "HLSR_TARGET_PLATFORM";
const PLATFORMS = ["win32", "linux"];

function targetPlatform(env) {
  const value = (env || process.env)[ENV];
  if (!value) return process.platform;
  if (PLATFORMS.indexOf(value) === -1) throw new Error(`${ENV}=${value}: поддерживаются ${PLATFORMS.join(", ")}`);
  return value;
}

// Сборка под другую ОС: байткод для неё компилирует её же Electron.
function isCross(env) {
  return targetPlatform(env) !== process.platform;
}

module.exports = { ENV, PLATFORMS, targetPlatform, isCross };
