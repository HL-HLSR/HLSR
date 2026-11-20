"use strict";

// Разрешение, в котором запустится Source-игра. Лаунчер передаёт -w/-h,
// только если разрешение задано в настройках игры; иначе движок берёт
// последнее сохранённое в реестре HKCU\Software\Valve\Source\<мод>\Settings
// (ScreenWidth/ScreenHeight). Под Proton этот раздел лежит в user.reg
// префикса. Предпросмотр HUD Ghosting по нему строит кадр нужной ширины.

const fs = require("fs");
const path = require("path");
const childProcess = require("child_process");

function clean(width, height) {
  const w = Number(width), h = Number(height);
  if (!Number.isInteger(w) || !Number.isInteger(h)) return null;
  if (w < 320 || h < 240 || w > 16384 || h > 16384) return null;
  return { width: w, height: h };
}

// user.reg Wine: «[Software\\Valve\\Source\\<мод>\\Settings] 1791620751»,
// затем строки «"ScreenWidth"=dword:000005a0» до пустой строки.
function fromUserReg(text, mod) {
  const header = `[Software\\\\Valve\\\\Source\\\\${mod}\\\\Settings]`.toLowerCase();
  const lines = String(text || "").split(/\r?\n/);
  const start = lines.findIndex((line) => line.toLowerCase().startsWith(header));
  if (start < 0) return null;
  const values = {};
  for (let i = start + 1; i < lines.length && !lines[i].startsWith("["); i++) {
    const match = /^"(ScreenWidth|ScreenHeight)"=dword:([0-9a-f]{1,8})\s*$/i.exec(lines[i]);
    if (match) values[match[1].toLowerCase()] = parseInt(match[2], 16);
  }
  return clean(values.screenwidth, values.screenheight);
}

// Вывод «reg query»: «    ScreenWidth    REG_DWORD    0xa00».
function fromRegQuery(text) {
  const values = {};
  for (const line of String(text || "").split(/\r?\n/)) {
    const match = /^\s*(ScreenWidth|ScreenHeight)\s+REG_DWORD\s+0x([0-9a-f]{1,8})\s*$/i.exec(line);
    if (match) values[match[1].toLowerCase()] = parseInt(match[2], 16);
  }
  return clean(values.screenwidth, values.screenheight);
}

function readUserReg(prefix, mod) {
  try {
    return fromUserReg(fs.readFileSync(path.join(prefix, "user.reg"), "utf8"), mod);
  } catch (error) {
    return null;
  }
}

function queryRegistry(mod, execFile = childProcess.execFile) {
  return new Promise((resolve) => {
    execFile("reg", ["query", `HKCU\\Software\\Valve\\Source\\${mod}\\Settings`], { windowsHide: true, timeout: 5000 }, (error, stdout) => {
      resolve(error ? null : fromRegQuery(stdout));
    });
  });
}

// { width, height, source: "launcher" | "game" } или null: тогда
// предпросмотр берёт разрешение монитора.
async function screenOf({ settings, mod, platform = process.platform, prefix = null, execFile } = {}) {
  const fromSettings = settings ? clean(settings.width, settings.height) : null;
  if (fromSettings) return Object.assign(fromSettings, { source: "launcher" });
  if (!/^[\w.-]+$/.test(String(mod || ""))) return null;
  let found = null;
  if (platform === "win32") found = await queryRegistry(mod, execFile);
  else if (prefix) found = readUserReg(prefix, mod);
  return found ? Object.assign(found, { source: "game" }) : null;
}

module.exports = { screenOf, fromUserReg, fromRegQuery };
