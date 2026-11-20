"use strict";

// Записи установки в HKCU через reg.exe. И чтение, и запись идут через
// .reg-файл в UTF-16LE (`reg export` / `reg import`): вывод `reg query`
// печатается в OEM-кодировке консоли, и кириллица в пути ломается.
const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const HEADER = "Windows Registry Editor Version 5.00";
const ROOTS = { HKCU: "HKEY_CURRENT_USER" };

function fullKey(key) {
  const [root, ...rest] = key.split("\\");
  if (!ROOTS[root]) throw new Error(`Неподдерживаемый раздел реестра: ${key}`);
  return [ROOTS[root]].concat(rest).join("\\");
}

function escape(value) {
  return String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function unescape(value) {
  return value.replace(/\\(.)/g, "$1");
}

function hexDword(value) {
  return `dword:${(Number(value) >>> 0).toString(16).padStart(8, "0")}`;
}

// changes: [{ key, values: { name: string | { dword: n } | null } } | { key, remove: true }]
function toRegFile(changes) {
  const lines = [HEADER, ""];
  for (const change of changes) {
    if (change.remove) {
      lines.push(`[-${fullKey(change.key)}]`, "");
      continue;
    }
    lines.push(`[${fullKey(change.key)}]`);
    for (const [name, value] of Object.entries(change.values || {})) {
      if (value === null) lines.push(`"${escape(name)}"=-`);
      else if (typeof value === "object") lines.push(`"${escape(name)}"=${hexDword(value.dword)}`);
      else lines.push(`"${escape(name)}"="${escape(value)}"`);
    }
    lines.push("");
  }
  return lines.join("\r\n");
}

// Строковые значения одного раздела из экспорта; подразделы не нужны.
function parseRegFile(text, key) {
  const header = `[${fullKey(key)}]`.toLowerCase();
  const values = {};
  let inside = false;
  for (const raw of text.replace(/^\uFEFF/, "").split(/\r?\n/)) {
    const line = raw.trim();
    if (line.startsWith("[")) {
      inside = line.toLowerCase() === header;
      continue;
    }
    if (!inside) continue;
    const match = /^"((?:[^"\\]|\\.)*)"="((?:[^"\\]|\\.)*)"$/.exec(line);
    if (match) values[unescape(match[1])] = unescape(match[2]);
  }
  return values;
}

function regExe(env) {
  return path.join(env.SystemRoot || env.WINDIR || "C:\\Windows", "System32", "reg.exe");
}

function tempFile(tempDir) {
  return path.join(tempDir || os.tmpdir(), `hlsr-reg-${crypto.randomBytes(6).toString("hex")}.reg`);
}

function createRegistry(options = {}) {
  const env = options.env || process.env;
  const run = options.run || ((args) => spawnSync(regExe(env), args, { windowsHide: true, encoding: "utf8", timeout: 30000 }));

  // Раздела нет — null; так же и при любой ошибке чтения.
  function read(key) {
    const file = tempFile(options.tempDir);
    try {
      const result = run(["export", key, file, "/y"]);
      if (result.status !== 0 || !fs.existsSync(file)) return null;
      return parseRegFile(fs.readFileSync(file, "utf16le"), key);
    } finally {
      fs.rmSync(file, { force: true });
    }
  }

  function apply(changes) {
    const file = tempFile(options.tempDir);
    try {
      fs.writeFileSync(file, "\uFEFF" + toRegFile(changes), "utf16le");
      const result = run(["import", file]);
      if (result.status !== 0)
        throw new Error(`reg import завершился с кодом ${result.status}: ${String(result.stderr || "").trim()}`);
    } finally {
      fs.rmSync(file, { force: true });
    }
  }

  return { read, apply };
}

module.exports = { createRegistry, toRegFile, parseRegFile };
