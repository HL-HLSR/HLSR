"use strict";

const path = require("path");
const os = require("os");

const DEFAULT_LIBRARY_PATH =
  process.platform === "win32" ? "C:\\HLSR" : path.join(os.homedir(), "Games", "HLSR");

module.exports = { DEFAULT_LIBRARY_PATH };
