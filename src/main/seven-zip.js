"use strict";

const fs = require("fs");
const path = require("path");

// Windows: свежий 7-Zip из resources/7zip. В 7zip-bin он застрял на 21.07 с известными уязвимостями
// декодеров, а архив .hlsrpack разбирается сразу после выбора файла. В сборке файл лежит в
// resources/7zip, при запуске из исходников — в resources/7zip/win-x64 репозитория.
// Linux: пока 7zip-bin.
function binPath(options = {}) {
  const platform = options.platform || process.platform;
  const resourcesPath = options.resourcesPath === undefined ? process.resourcesPath : options.resourcesPath;

  if (platform !== "win32") {
    const sevenBin = require("7zip-bin");
    return sevenBin.path7za.replace("app.asar" + path.sep, "app.asar.unpacked" + path.sep);
  }

  const packaged = resourcesPath ? path.join(resourcesPath, "7zip", "7za.exe") : null;
  if (packaged && fs.existsSync(packaged)) return packaged;
  return path.resolve(__dirname, "..", "..", "resources", "7zip", "win-x64", "7za.exe");
}

module.exports = { binPath };
