"use strict";

// Копия scripts/lib/launcher-changelog.js 0.4.7 (подпроект 6, задача 8) с правкой источника:
// записи RELEASES берутся не из renderer/components/Launcher/changelog.js (Vue удалён), а из
// crates/hlsr-ui/data/changelog.json (поле releases: [{version, ru[], en[]}], его пишет
// tools/reference/export-changelog.js и дополняет выпуск). Другой файл — HLSR_CHANGELOG_JSON
// (тесты). Правила те же: запись для выпускаемой версии обязательна, обе локали, поровну пунктов.

const fs = require("fs");
const path = require("path");
const common = require("./release-common");

const SCHEMA_VERSION = 1;
const LANGUAGES = ["ru", "en"];
const SOURCE = path.join(common.REPO_ROOT, "crates", "hlsr-ui", "data", "changelog.json");

function releases(source) {
  const file = source || process.env.HLSR_CHANGELOG_JSON || SOURCE;
  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!Array.isArray(data.releases)) throw new Error(`${file}: нет списка releases`);
  return { list: data.releases, file };
}

function releaseFor(version, source) {
  return releases(source).list.filter((release) => release.version === version)[0] || null;
}

function buildChangelogJson(version, generatedAt, source) {
  if (!version || typeof version !== "string")
    throw new Error("Версия выпуска не передана");

  const { file } = releases(source);
  const release = releaseFor(version, source);

  if (!release)
    throw new Error(
      `В ${file} нет записи для ${version}. ` +
        "Ченджлог выпускаемой версии обязателен: допишите releases обеими локалями."
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

module.exports = { buildChangelogJson, releaseFor, SCHEMA_VERSION, LANGUAGES, SOURCE };
