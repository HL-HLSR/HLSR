"use strict";

const fs = require("fs");
const path = require("path");

function writeFileAtomic(file, contents) {
  const dir = path.dirname(file);
  fs.mkdirSync(dir, { recursive: true });

  const temp = path.join(dir, `.${path.basename(file)}.${process.pid}.${Date.now().toString(36)}.tmp`);
  const handle = fs.openSync(temp, "w");

  try {
    fs.writeFileSync(handle, contents, "utf8");
    fs.fsyncSync(handle);
  } finally {
    fs.closeSync(handle);
  }

  try {
    fs.renameSync(temp, file);
  } catch (error) {
    try {
      fs.rmSync(temp, { force: true });
    } catch (e) {
      /* временный файл — не повод падать */
    }
    throw error;
  }
}

function writeJsonAtomic(file, data) {
  writeFileAtomic(file, JSON.stringify(data, null, 2) + "\n");
}

function readJsonState(file) {
  let raw;

  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (error) {
    if (error && error.code === "ENOENT") return { found: false, valid: false, data: null };
    return { found: true, valid: false, data: null, reason: error.message };
  }

  try {
    return { found: true, valid: true, data: JSON.parse(raw.charCodeAt(0) === 0xfeff ? raw.slice(1) : raw) };
  } catch (error) {
    return { found: true, valid: false, data: null, reason: error.message };
  }
}

function readJsonSafe(file) {
  return readJsonState(file).data;
}

module.exports = { writeFileAtomic, writeJsonAtomic, readJsonSafe, readJsonState };
