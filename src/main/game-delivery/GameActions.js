"use strict";

const fs = require("fs");
const path = require("path");

const manifestRegistry = require("./manifests");
const { DeliveryError } = require("./errors");
const { assertInside, gameInstallPath } = require("./paths");

const { desktopExecPath } = require("../linux/DesktopIntegration");

const SELECT_GAME_ARG = "--select-game=";

class GameActions {
  constructor(options) {
    const opts = options || {};

    this.paths = opts.paths;
    this.electron = opts.electron || require("electron");
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.installService = opts.installService;
  }

  resolveGameId(rawGameId) {
    const gameId = String(rawGameId || "");

    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(gameId) || !manifestRegistry.isKnownGame(gameId))
      throw new DeliveryError("PATH_INVALID", `Неизвестная игра: ${rawGameId}`);

    return gameId;
  }

  installPathOf(gameId) {
    const installPath = gameInstallPath(this.paths, gameId);

    if (!fs.existsSync(path.join(installPath, "installed.json")))
      throw new DeliveryError("NOT_INSTALLED", "Игра не установлена");

    return installPath;
  }

  resolveOpenTarget(gameId, relativePath, options) {
    const opts = options || {};
    const installPath = this.installPathOf(gameId);

    if (!relativePath) return installPath;

    const target = assertInside(
      installPath,
      path.join(installPath, String(relativePath).replace(/\//g, path.sep)),
      `Папка "${relativePath}"`
    );

    assertInside(this.paths.root, target, "Путь библиотеки");

    if (!fs.existsSync(target)) {
      if (!opts.create)
        throw new DeliveryError("PATH_INVALID", `Папка не найдена: ${relativePath}`);

      fs.mkdirSync(target, { recursive: true });
      this.logger.info(`[game-delivery] создана папка ${target}`);
    }

    return target;
  }

  async openPath(target) {
    const error = await this.electron.shell.openPath(target);

    if (error)
      throw new DeliveryError("PATH_INVALID", `Не удалось открыть папку: ${error}`, { path: target });

    return target;
  }

  async openGameFolder(rawGameId) {
    const gameId = this.resolveGameId(rawGameId);
    const target = this.resolveOpenTarget(gameId, null);

    await this.openPath(target);
    return { opened: target };
  }

  async openScreenshotsFolder(rawGameId) {
    const gameId = this.resolveGameId(rawGameId);
    const config = manifestRegistry.localConfigOf(gameId);

    if (!config || !config.screenshotsPath)
      throw new DeliveryError("PATH_INVALID", "Для этой сборки не задана папка скриншотов");

    const target = this.resolveOpenTarget(gameId, config.screenshotsPath, { create: true });

    await this.openPath(target);
    return { opened: target, created: true };
  }

  createDesktopShortcut(rawGameId) {
    if (process.platform === "linux") return this.createDesktopEntry(rawGameId);

    const gameId = this.resolveGameId(rawGameId);
    const config = manifestRegistry.localConfigOf(gameId) || {};
    const { app, shell } = this.electron;

    const desktop = app.getPath("desktop");
    const name = config.shortcutName || `HLSR - ${gameId}`;
    const linkPath = path.join(desktop, `${name}.lnk`);

    const target = process.execPath;
    const args = `${SELECT_GAME_ARG}${gameId}`;

    const ok = shell.writeShortcutLink(linkPath, "create", {
      target,
      args,
      description: name,
      cwd: path.dirname(target),
      icon: target,
      iconIndex: 0,
    });

    if (!ok)
      throw new DeliveryError("PATH_NOT_WRITABLE", "Не удалось создать ярлык на рабочем столе");

    this.logger.info(`[game-delivery] создан ярлык ${linkPath} (${args})`);
    return { shortcut: linkPath, args };
  }

  // Linux: ярлык — .desktop-файл, который запускает лаунчер через start
  // архивной установки, AppImage или текущий процесс в режиме разработки.
  createDesktopEntry(rawGameId) {
    const gameId = this.resolveGameId(rawGameId);
    const config = manifestRegistry.localConfigOf(gameId) || {};
    const { app } = this.electron;

    const desktop = app.getPath("desktop");
    const name = config.shortcutName || `HLSR - ${gameId}`;
    const linkPath = this.shortcutPathOf(gameId);
    const installRoot = require("../launcher-update/ArchiveInstall").managedRoot(process.execPath);
    const target = installRoot ? path.join(installRoot, "start") : process.env.APPIMAGE || process.execPath;
    const args = `${SELECT_GAME_ARG}${gameId}`;
    const quotedTarget = desktopExecPath(target);
    if (!quotedTarget) throw new DeliveryError("PATH_INVALID", "Некорректный путь лаунчера");
    const appArg = !process.env.APPIMAGE && !app.isPackaged
      ? desktopExecPath(app.getAppPath()) : null;
    const execArgs = [quotedTarget, appArg, "--no-sandbox", args].filter(Boolean).join(" ");
    const content = [
      "[Desktop Entry]",
      "Type=Application",
      `Name=${name.replace(/[\r\n]/g, " ")}`,
      `Exec=${execArgs}`,
      "Icon=applications-games",
      "Categories=Game;",
      `X-HLSR-GameId=${gameId}`,
      "",
    ].join("\n");

    try {
      fs.mkdirSync(desktop, { recursive: true });
      fs.writeFileSync(linkPath, content, { mode: 0o755 });
      fs.chmodSync(linkPath, 0o755);
    } catch (error) {
      throw new DeliveryError("PATH_NOT_WRITABLE", `Не удалось создать ярлык: ${error.message}`);
    }

    this.logger.info(`[game-delivery] создан ярлык ${linkPath} (${args})`);
    return { shortcut: linkPath, args };
  }

  shortcutPathOf(gameId) {
    if (process.platform === "linux")
      return path.join(this.electron.app.getPath("desktop"), `hlsr-${gameId}.desktop`);

    const config = manifestRegistry.localConfigOf(gameId) || {};
    const name = config.shortcutName || `HLSR - ${gameId}`;

    return path.join(this.electron.app.getPath("desktop"), `${name}.lnk`);
  }

  removeDesktopShortcut(rawGameId) {
    const gameId = this.resolveGameId(rawGameId);
    const linkPath = this.shortcutPathOf(gameId);

    if (!fs.existsSync(linkPath)) return { removed: false, reason: "absent" };

    let ours;
    try {
      ours = this.isOwnShortcut(linkPath, gameId);
    } catch (error) {
      return { removed: false, reason: "unreadable" };
    }

    if (!ours) return { removed: false, reason: "foreign" };

    try {
      fs.unlinkSync(linkPath);
    } catch (error) {
      this.logger.warn(`[game-delivery] не удалось удалить ярлык ${linkPath}: ${error.message}`);
      return { removed: false, reason: "failed" };
    }

    this.logger.info(`[game-delivery] удалён ярлык ${linkPath}`);
    return { removed: true, path: linkPath };
  }

  isOwnShortcut(linkPath, gameId) {
    if (process.platform === "linux") {
      const content = fs.readFileSync(linkPath, "utf8");
      return content.includes(`X-HLSR-GameId=${gameId}\n`) && content.includes(`${SELECT_GAME_ARG}${gameId}`);
    }

    const link = this.electron.shell.readShortcutLink(linkPath);

    return (
      !!link &&
      String(link.target || "").toLowerCase() === String(process.execPath).toLowerCase() &&
      String(link.args || "").indexOf(`${SELECT_GAME_ARG}${gameId}`) !== -1
    );
  }

  static parseSelectGameArg(argv) {
    for (const arg of argv || []) {
      const value = String(arg);
      if (!value.startsWith(SELECT_GAME_ARG)) continue;

      const gameId = value.slice(SELECT_GAME_ARG.length);
      if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(gameId)) return null;

      return gameId;
    }

    return null;
  }
}

module.exports = GameActions;
module.exports.SELECT_GAME_ARG = SELECT_GAME_ARG;
