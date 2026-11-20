"use strict";
const { contextBridge, ipcRenderer } = require("electron");
contextBridge.exposeInMainWorld("installer", {
  state: () => ipcRenderer.invoke("installer-get-state"),
  install: (desktop) => ipcRenderer.invoke("installer-install", desktop === true),
  cancel: () => ipcRenderer.invoke("installer-cancel"),
  selectDirectory: () => ipcRenderer.invoke("installer-select-directory"),
  uninstallPlan: () => ipcRenderer.invoke("installer-uninstall-plan"),
  uninstall: (removeGames) => ipcRenderer.invoke("installer-uninstall", removeGames === true),
  windowCommand: (command) => ipcRenderer.invoke("installer-window", command),
  launch: () => ipcRenderer.invoke("installer-launch"),
  back: () => ipcRenderer.invoke("installer-back"),
  openDiscord: () => ipcRenderer.invoke("installer-open-discord"),
  onState: (callback) => ipcRenderer.on("installer-state", (_event, state) => callback(state)),
});
