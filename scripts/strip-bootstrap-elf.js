"use strict";

const fs = require("fs");
const path = require("path");
const stripLinuxElf = require("./strip-linux-elf");

module.exports = function stripBootstrapElf(context) {
  stripLinuxElf(context);
  if (context.electronPlatformName !== "linux") return;
  const locales = path.join(context.appOutDir, "locales");
  const keep = new Set(["en-US.pak", "en-GB.pak", "ru.pak"]);
  let removed = 0;
  for (const name of fs.readdirSync(locales)) {
    if (keep.has(name)) continue;
    if (!name.endsWith(".pak")) throw new Error(`Неожиданный файл локали: ${name}`);
    fs.unlinkSync(path.join(locales, name));
    removed++;
  }
  console.log(`[bootstrap] Сохранены локали en-US, en-GB, ru; удалено ${removed}`);
};
