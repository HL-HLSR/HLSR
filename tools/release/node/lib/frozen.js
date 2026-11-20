"use strict";

// Модули 0.4.7, которыми подписывается и проверяется канал: замороженные копии в tools/compat-047
// (дословные, сверены с эталоном; src/ лаунчера 0.5 этих файлов больше не содержит). Подпись и
// проверка идут тем же кодом, каким канал проверит обновлялка 0.4.7 у пользователя, поэтому
// скрипты выпуска берут именно их, а не отдельную реализацию.

const path = require("path");

const SRC = path.resolve(__dirname, "..", "..", "..", "compat-047", "src");

const load = (relative) => require(path.join(SRC, ...relative.split("/")));

module.exports = {
  SRC,
  signing: load("main/game-delivery/manifest-signing.js"),
  launcherChannel: load("main/launcher-update/launcher-channel.js"),
  chunks: load("main/launcher-update/chunks.js"),
  endpoints: load("shared/delivery-endpoints.js"),
  load,
};
