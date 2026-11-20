#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync, spawn } = require("child_process");

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--proton") out.proton = path.resolve(argv[++i]);
    else if (argv[i] === "--livesplit") out.livesplit = path.resolve(argv[++i]);
    else if (argv[i] === "--umu") out.umu = path.resolve(argv[++i]);
    else throw new Error(`Неизвестный аргумент: ${argv[i]}`);
  }
  return out;
}

function sleep(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }

function x11Windows() {
  const root = execFileSync("xprop", ["-root", "_NET_CLIENT_LIST_STACKING"], { encoding: "utf8" });
  const ids = (root.split("# ")[1] || "").split(",").map((item) => item.trim()).filter(Boolean);
  return ids.map((id) => {
    let title = "";
    try {
      title = execFileSync("xprop", ["-id", id, "WM_NAME", "_NET_WM_NAME"],
        { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    } catch (error) { /* window disappeared */ }
    return { id, title };
  });
}

function terminate(child) {
  if (!child || !child.pid) return;
  try { process.kill(-child.pid, "SIGTERM"); } catch (error) { /* already stopped */ }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.proton || !args.livesplit || !args.umu)
    throw new Error("Нужны --proton, --livesplit и --umu");
  if (!process.env.WAYLAND_DISPLAY || !process.env.DISPLAY)
    throw new Error("Для smoke-теста нужны одновременно Wayland и XWayland");
  for (const file of [path.join(args.proton, "proton"), args.livesplit, args.umu])
    if (!fs.existsSync(file)) throw new Error(`Файл не найден: ${file}`);

  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-mixed-driver-smoke-"));
  const prefix = path.join(temporary, "pfx");
  const base = Object.assign({}, process.env, {
    WINEPREFIX: prefix,
    PROTONPATH: args.proton,
    PROTON_VERB: "run",
    GAMEID: "0",
    STORE: "none",
  });
  const children = [];
  try {
    const before = new Set(x11Windows().map((window) => window.id));
    const game = spawn(args.umu, ["C:\\windows\\syswow64\\notepad.exe"], {
      env: Object.assign({}, base, { PROTON_ENABLE_WAYLAND: "1" }),
      detached: true,
      stdio: "ignore",
    });
    children.push(game);
    await sleep(7000);
    if (game.exitCode != null) throw new Error(`Wayland-процесс завершился с кодом ${game.exitCode}`);
    const afterGame = x11Windows().filter((window) => !before.has(window.id));
    if (afterGame.some((window) => /notepad|блокнот/i.test(window.title)))
      throw new Error("Контрольный процесс оказался в XWayland вместо Wayland");

    const timer = spawn(args.umu, [args.livesplit], {
      cwd: path.dirname(args.livesplit),
      env: Object.assign({}, base, {
        PROTON_ENABLE_WAYLAND: "1",
        PROTON_USE_X11_EXCLUSIVE: "LiveSplit.exe",
      }),
      detached: true,
      stdio: "ignore",
    });
    children.push(timer);
    let liveSplitWindow = null;
    for (let attempt = 0; attempt < 30 && !liveSplitWindow; attempt++) {
      await sleep(500);
      liveSplitWindow = x11Windows().find((window) =>
        !before.has(window.id) && /livesplit/i.test(window.title));
    }
    if (!liveSplitWindow) throw new Error("LiveSplit не создал XWayland-окно после Wayland-процесса");
    if (game.exitCode != null) throw new Error("Wayland-процесс завершился во время запуска LiveSplit");
    process.stdout.write(`OK: game=Wayland, LiveSplit=XWayland (${liveSplitWindow.id})\n`);
  } finally {
    for (const child of children.reverse()) terminate(child);
    await sleep(500);
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

main().catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
