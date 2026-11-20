"use strict";

// Ярлыки HLSR в меню «Пуск» и на рабочем столе — те же файлы HLSR.lnk,
// что создавал прежний установщик, поэтому обновление их подхватывает.
const fs = require("fs");
const path = require("path");
const layout = require("./layout");

function createShortcuts({ shell, env = process.env, desktopDir, identity }) {
  const startMenu = path.join(env.APPDATA || "", "Microsoft", "Windows", "Start Menu", "Programs", `${identity.shortcutName}.lnk`);
  const desktop = path.join(desktopDir, `${identity.shortcutName}.lnk`);

  function write(file, exe) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const ok = shell.writeShortcutLink(file, "create", {
      target: exe, cwd: path.dirname(exe), icon: exe, iconIndex: 0,
      appUserModelId: identity.appId, description: identity.productName,
    });
    if (!ok) throw new Error(`Не удалось создать ярлык ${file}`);
  }

  // Удаляем только свои ярлыки: чужой HLSR.lnk остаётся на месте.
  function pointsTo(file, exe) {
    try {
      return layout.isSameDir(shell.readShortcutLink(file).target, exe);
    } catch (_) {
      return false;
    }
  }

  return { startMenu, desktop, write, pointsTo, exists: (file) => fs.existsSync(file) };
}

module.exports = { createShortcuts };
