"use strict";

const fs = require("fs");
const path = require("path");
const { channel } = require("../build-channel.json");

// Ключи QA-канала. Маркер лежит в resources рядом с app.asar: сам апдейтер внутри asar
// (src/main/launcher-update/updater-mode.js), туда файл не положить. Действует только в QA-сборке:
// канал сборки лежит в asar (scripts/lib/updater-bundle.js), а файл рядом с ним может положить
// другой пользователь этого компьютера.
function read(resourcesPath) {
  if (channel !== "qa") return null;

  const marker = path.join(resourcesPath || "", "QA_TRUST");

  try {
    if (!fs.existsSync(marker)) return null;

    const data = JSON.parse(fs.readFileSync(marker, "utf8"));
    if (!data || !Array.isArray(data.publicKeys) || !data.publicKeys.length) return null;

    return {
      publicKeys: data.publicKeys,
      downloadBase: typeof data.downloadBase === "string" ? data.downloadBase : null,
    };
  } catch (error) {
    return null;
  }
}

module.exports = { read };
