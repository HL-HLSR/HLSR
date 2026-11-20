"use strict";

const fs = require("fs");
const path = require("path");

const bin = require("7zip-bin").path7za;
const expected = path.resolve(__dirname, "..", "node_modules", "7zip-bin", "linux", "x64", "7za");
if (process.platform !== "linux" || path.resolve(bin) !== expected || !fs.statSync(bin).isFile())
  throw new Error("Не найден локальный Linux x64 7za");

fs.chmodSync(bin, fs.statSync(bin).mode | 0o111);
process.stdout.write(`Готов Linux-распаковщик: ${bin}\n`);
