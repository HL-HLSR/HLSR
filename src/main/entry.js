"use strict";

// Точка входа main-бандла. Один и тот же app.asar запускается и как
// лаунчер, и как внешний апдейтер Windows (launcher-update/updater-mode.js).
// index.js подключается только в режиме лаунчера: его модули при загрузке
// уже настраивают логи, Chromium и IPC.
const updaterMode = require("./launcher-update/updater-mode");

if (updaterMode.isUpdaterInvocation(process.argv)) updaterMode.run();
else require("./index");
