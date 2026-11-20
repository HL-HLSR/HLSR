"use strict";

const path = require("path");

const SAFETY_RETURN_MS = 60 * 60 * 1000;
// Окно возвращается не в ту же секунду, когда игра закрылась: если за это
// время игра запустилась снова или ещё идёт другая, окно остаётся в трее и
// не забирает фокус у игры.
const FINISH_SETTLE_MS = 3000;

class TrayController {
  constructor(options) {
    const opts = options || {};

    this.window = opts.window || null;
    this.electron = opts.electron || require("electron");
    this.logger = opts.logger || { info() {}, warn() {} };
    this.translate = opts.translate || ((key) => key);
    this.iconPath = opts.iconPath || null;
    // Идёт ли сейчас хоть одна игра: страховка и возврат окна сверяются с ним.
    this.isGameRunning = typeof opts.isGameRunning === "function" ? opts.isGameRunning : () => false;
    this.settleMs = opts.settleMs == null ? FINISH_SETTLE_MS : opts.settleMs;
    this.settleTimer = null;

    this.tray = null;
    this.hidden = false;
    this.safetyTimer = null;

    this.enabled = true;
  }

  setEnabled(value) {
    this.enabled = value !== false;

    if (!this.enabled && this.hidden) this.show();
  }

  onGameStarted() {
    this.cancelSettle();
    if (!this.enabled) return;
    this.hide();
  }

  onGameFinished() {
    if (!this.hidden) return;
    this.cancelSettle();
    this.settleTimer = setTimeout(() => {
      this.settleTimer = null;
      if (this.stillPlaying()) return;
      this.show();
    }, this.settleMs);
    if (typeof this.settleTimer.unref === "function") this.settleTimer.unref();
  }

  cancelSettle() {
    if (this.settleTimer) clearTimeout(this.settleTimer);
    this.settleTimer = null;
  }

  stillPlaying() {
    try {
      return !!this.isGameRunning();
    } catch (error) {
      return false;
    }
  }

  hide() {
    if (this.hidden) return;
    if (!this.window || this.window.isDestroyed()) return;

    if (!this.ensureTray()) {
      this.logger.warn("[tray] иконка не создана, окно остаётся на экране");
      return;
    }

    this.hidden = true;
    this.window.hide();

    this.armSafety();
  }

  // Страховка на случай, если событие о закрытии игры потерялось. Раньше
  // она через час возвращала окно с фокусом прямо поверх идущей игры.
  armSafety() {
    if (this.safetyTimer) clearTimeout(this.safetyTimer);
    this.safetyTimer = setTimeout(() => {
      this.safetyTimer = null;
      if (this.stillPlaying()) {
        this.armSafety();
        return;
      }
      this.logger.warn("[tray] событие о завершении игры не пришло, возвращаю окно");
      this.show();
    }, SAFETY_RETURN_MS);

    if (typeof this.safetyTimer.unref === "function") this.safetyTimer.unref();
  }

  show() {
    this.cancelSettle();
    if (this.safetyTimer) {
      clearTimeout(this.safetyTimer);
      this.safetyTimer = null;
    }

    this.hidden = false;

    if (!this.window || this.window.isDestroyed()) return;

    try {
      if (this.window.isMinimized()) this.window.restore();
      this.window.show();
      this.window.focus();
    } catch (error) {
      this.logger.warn(`[tray] окно не показано: ${error.message}`);
    }

    this.destroyTray();
  }

  resolveIconPath() {
    if (this.iconPath) return this.iconPath;

    const app = this.electron && this.electron.app;
    const root = app && typeof app.getAppPath === "function" ? app.getAppPath() : null;

    return root
      ? path.join(root, "icons", "hlsrc.png")
      : path.join(__dirname, "..", "..", "..", "icons", "hlsrc.png");
  }

  ensureTray() {
    if (this.tray) return true;

    const { Tray, Menu, nativeImage } = this.electron;
    if (!Tray || !Menu) return false;

    try {
      const file = this.resolveIconPath();
      const image = nativeImage ? nativeImage.createFromPath(file) : file;

      if (image && typeof image.isEmpty === "function" && image.isEmpty()) {
        this.logger.warn(`[tray] иконка не найдена: ${file}`);
        this.tray = null;
        return false;
      }

      this.tray = new Tray(image);

      this.tray.setToolTip("HLSR");
      this.tray.setContextMenu(
        Menu.buildFromTemplate([
          { label: this.translate("#L_TRAY_SHOW"), click: () => this.show() },
          { type: "separator" },
          {
            label: this.translate("#L_TRAY_QUIT"),
            click: () => {
              const app = this.electron.app;
              if (app) app.quit();
            },
          },
        ])
      );

      this.tray.on("click", () => this.show());
      this.tray.on("double-click", () => this.show());

      return true;
    } catch (error) {
      this.logger.warn(`[tray] иконка не создана: ${error.message}`);
      this.tray = null;
      return false;
    }
  }

  destroyTray() {
    if (!this.tray) return;

    try {
      this.tray.destroy();
    } catch (error) {
      this.logger.warn(`[tray] иконка не снята: ${error.message}`);
    }

    this.tray = null;
  }

  dispose() {
    this.cancelSettle();
    if (this.safetyTimer) clearTimeout(this.safetyTimer);
    this.safetyTimer = null;
    this.destroyTray();
  }
}

module.exports = TrayController;
module.exports.SAFETY_RETURN_MS = SAFETY_RETURN_MS;
module.exports.FINISH_SETTLE_MS = FINISH_SETTLE_MS;
