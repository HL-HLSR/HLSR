"use strict";

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

// electron-builder calls this after packing, before creating the AppImage.
// Electron's main binary is already stripped; only touch x86_64 ELF files
// that still carry a regular symbol table or debug sections.
module.exports = function stripLinuxElf(context) {
  if (context.electronPlatformName !== "linux") return;

  let files = 0;
  let saved = 0;
  const pending = [context.appOutDir];
  while (pending.length) {
    const directory = pending.pop();
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name);
      if (entry.isDirectory()) { pending.push(file); continue; }
      if (!entry.isFile()) continue;

      const fd = fs.openSync(file, "r");
      const header = Buffer.alloc(20);
      let bytes;
      try { bytes = fs.readSync(fd, header, 0, header.length, 0); }
      finally { fs.closeSync(fd); }
      if (bytes < 20 || header.subarray(0, 4).toString("hex") !== "7f454c46" ||
          header[4] !== 2 || header[5] !== 1 || header.readUInt16LE(18) !== 62) continue;

      const sections = execFileSync("readelf", ["-S", file], { encoding: "utf8" });
      if (!/\.(?:debug_[\w.]+|symtab)\b/.test(sections)) continue;
      const before = fs.statSync(file).size;
      execFileSync("strip", ["--strip-unneeded", file]);
      saved += before - fs.statSync(file).size;
      files++;
    }
  }
  console.log(`[strip] Linux x64 ELF: ${files} файлов, удалено ${saved} байт`);
};
