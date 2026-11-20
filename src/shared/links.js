"use strict";

const LINKS = {
  site: "https://hlsr.pro",
  privacy: "https://hlsr.pro/privacy/",
  terms: "https://hlsr.pro/terms/",
  // Исходники и страница выпусков.
  github: "https://github.com/HL-HLSR/HLSR",
  discord: "https://discord.com/invite/8Np6brWBuz",
  steamHalfLife: "https://store.steampowered.com/app/70/HalfLife/",
  steamHalfLife2: "https://store.steampowered.com/app/220/HalfLife_2/",
  // Откуда лаунчер берёт паки HUD и прицелов HL1 (src/main/game-delivery/goldsrc-packs.js).
  hl1Resources: "https://www.speedrun.com/hl1/resources",
  // Исходник bxt-rs (GPL-3), которую «Демо» скачивает для рендера HL1 (config/tools/bxt-rs.manifest.json).
  bxtrsSource: "https://hlsr.pro/downloads/tools/bxt-rs/won-2623912/bxt-rs-won-2623912-src.tar.gz",
  // Её лицензия и уведомления крейтов (те же файлы ставятся рядом с DLL).
  bxtrsLicense: "https://hlsr.pro/downloads/tools/bxt-rs/won-2623912/THIRD-PARTY-NOTICES.txt",
  "speedrun-hl1": "https://www.speedrun.com/hl1",
  "speedrun-hl2": "https://www.speedrun.com/hl2",
  "google-drive": "https://drive.google.com/drive/my-drive",
  authorInfiniteCore: "https://github.com/InfiniteC0re",
  authorProlert: "https://github.com/prolert",
  authorKanoyo: "https://github.com/kanoyo-git",
  authorScriptedSnark: "https://github.com/ScriptedSnark",
};

const KEYS = Object.keys(LINKS);

function urlFor(key) {
  if (typeof key !== "string") return null;
  return Object.prototype.hasOwnProperty.call(LINKS, key) ? LINKS[key] : null;
}

function isKnownKey(key) {
  return urlFor(key) !== null;
}

module.exports = { LINKS, KEYS, urlFor, isKnownKey };
