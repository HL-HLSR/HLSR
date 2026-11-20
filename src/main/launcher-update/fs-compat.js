"use strict";

// Electron patches the built-in fs module so that *.asar paths behave like
// directories. Distribution and removal code needs the actual file instead.
// In regular Node processes (tests and build scripts), fall back to normal fs.
// eval("require") keeps webpack from resolving the Electron-only module while
// bundling the main process.
function load() {
  try {
    const dynamicRequire = eval("require");
    return dynamicRequire("original-fs");
  } catch (_) {
    return require("fs");
  }
}

module.exports = load();
