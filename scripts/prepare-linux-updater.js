"use strict";
const fs = require("fs");
const path = require("path");
const endpoints = require("../src/shared/delivery-endpoints");
const target = path.join(__dirname, "../build/linux-app-update.yml");
fs.mkdirSync(path.dirname(target), { recursive: true });
fs.writeFileSync(target, `provider: generic\nurl: ${endpoints.launcherFor("linux").feedUrls[0]}\nupdaterCacheDirName: hlsr-linux-updater\n`);
