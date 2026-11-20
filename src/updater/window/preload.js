"use strict";

const { contextBridge, ipcRenderer } = require("electron");

const SEND_CHANNELS = new Set(["updater:cancel", "updater:close"]);
const EVENT_CHANNELS = new Set(["updater:init", "updater:progress", "updater:stage"]);

contextBridge.exposeInMainWorld("hlsrUpdaterIpc", {
  send(channel) {
    if (!SEND_CHANNELS.has(channel)) throw new Error(`Blocked updater IPC: ${String(channel)}`);
    ipcRenderer.send(channel);
  },
  on(channel, listener) {
    if (!EVENT_CHANNELS.has(channel)) throw new Error(`Blocked updater IPC: ${String(channel)}`);
    if (typeof listener !== "function") throw new TypeError("IPC listener must be a function");
    ipcRenderer.on(channel, (_event, payload) => listener(undefined, payload));
  },
});
