"use strict";

const path = require("path");
const { app, dialog } = require("electron");
const { openInstaller } = require("../main/launcher-update/BootstrapInstaller");

let window = null;
app.setPath("userData", path.join(app.getPath("userData"), "installer"));
if (process.platform === "linux") app.setDesktopName("hlsr.desktop");
require("../main/gpu-preference").apply(app);

function showError(error) {
  const message = String((error && (error.stack || error.message)) || error).slice(0, 600);
  dialog.showErrorBox("HLSR — ошибка установки / installer error", message);
  app.exit(1);
}

app.on("ready", () => {
  try {
    if (!app.requestSingleInstanceLock()) { app.quit(); return; }
    window = openInstaller(require("electron"));
    app.on("second-instance", () => { if (window) { window.show(); window.focus(); } });
  } catch (error) { showError(error); }
});
app.on("window-all-closed", () => { window = null; app.quit(); });
app.on("activate", () => {
  if (!window) {
    try { window = openInstaller(require("electron")); }
    catch (error) { showError(error); }
  }
});
process.on("uncaughtException", showError);
process.on("unhandledRejection", showError);
