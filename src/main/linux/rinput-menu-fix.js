"use strict";

// Linux: патч курсора RInput в меню (см. game-delivery/rinput-menu-fix.js)
// через /proc/<pid>/mem. Wine грузит DLL как обычные отображения файлов,
// поэтому базы модулей видны в /proc/<pid>/maps.

const nodeFs = require("fs");
const core = require("../game-delivery/rinput-menu-fix");

function parseMaps(text) {
  return text.split("\n").filter(Boolean).map((line) => {
    const parts = line.trim().split(/\s+/);
    const [start] = parts[0].split("-");
    return { start: parseInt(start, 16), offset: parseInt(parts[2], 16), file: parts.slice(5).join(" ") };
  });
}

function imageBase(maps, name) {
  const pattern = new RegExp(`[\\\\/]${name.replace(".", "\\.")}$`, "i");
  const entry = maps.find((m) => m.offset === 0 && pattern.test(m.file));
  return entry ? entry.start : null;
}

// Файл памяти открывается при первом чтении: пока RInput не загружен,
// процесс трогать незачем.
class ProcessMemory {
  constructor(pid, fs) {
    this.pid = pid;
    this.fs = fs;
    this.fd = null;
    this.maps = parseMaps(fs.readFileSync(`/proc/${pid}/maps`, "utf8"));
  }
  moduleBase(name) { return imageBase(this.maps, name); }
  file() {
    if (this.fd == null) this.fd = this.fs.openSync(`/proc/${this.pid}/mem`, "r+");
    return this.fd;
  }
  read(address, length) {
    const buf = Buffer.alloc(length);
    this.fs.readSync(this.file(), buf, 0, length, address);
    return buf;
  }
  write(address, buf) { this.fs.writeSync(this.file(), buf, 0, buf.length, address); }
  close() { if (this.fd != null) this.fs.closeSync(this.fd); }
}

async function patchProcess(pid, fs = nodeFs) {
  const mem = new ProcessMemory(pid, fs);
  try {
    return await core.patchMemory(mem);
  } finally {
    mem.close();
  }
}

// Патчит все запущенные процессы игры с таким именем (Wine называет их по exe).
async function patchRunningGame(processName, fs = nodeFs) {
  let result = "waiting";
  for (const entry of fs.readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    let comm;
    try { comm = fs.readFileSync(`/proc/${entry}/comm`, "utf8").trim().toLowerCase(); } catch (error) { continue; }
    if (comm !== processName.toLowerCase()) continue;
    result = core.mergeStatus(result, await patchProcess(entry, fs));
  }
  return result;
}

module.exports = {
  patchProcess,
  patchRunningGame,
  parseMaps,
  buildCave: core.buildCave,
  originalHook: core.originalHook,
  exportAddress: core.exportAddress,
  CAVE: core.CAVE,
  HOOK: core.HOOK,
  STUB: core.STUB,
  TRAMPOLINE: core.TRAMPOLINE,
};
