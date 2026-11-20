"use strict";

const changelogModule = require("../../src/renderer/components/Launcher/changelog");

const changelog = changelogModule.default || changelogModule;

const SCHEMA_VERSION = 1;
const LANGUAGES = ["ru", "en"];

function releaseFor(version) {
  return changelog.RELEASES.filter((release) => release.version === version)[0] || null;
}

function buildChangelogJson(version, generatedAt) {
  if (!version || typeof version !== "string")
    throw new Error("Версия выпуска не передана");

  const release = releaseFor(version);

  if (!release)
    throw new Error(
      `В src/renderer/components/Launcher/changelog.js нет записи для ${version}. ` +
        "Ченджлог выпускаемой версии обязателен: допишите RELEASES обеими локалями " +
        "и пересоберите лаунчер."
    );

  const items = {};

  for (const lang of LANGUAGES) {
    const list = (release[lang] || []).filter((item) => typeof item === "string" && item.trim());

    if (!list.length)
      throw new Error(`У выпуска ${version} пустой ченджлог на языке «${lang}»`);

    items[lang] = list.map((item) => item.trim());
  }

  if (items.ru.length !== items.en.length)
    throw new Error(
      `У выпуска ${version} разное число пунктов: ru ${items.ru.length}, en ${items.en.length}`
    );

  return Object.assign(
    {
      schemaVersion: SCHEMA_VERSION,
      version,
      generatedAt: generatedAt || new Date().toISOString(),
    },
    items
  );
}

module.exports = { buildChangelogJson, releaseFor, SCHEMA_VERSION, LANGUAGES };
