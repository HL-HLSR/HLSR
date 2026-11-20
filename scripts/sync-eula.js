#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const BUILD_DIR = path.resolve(__dirname, "..", "build");
const BOM = "﻿";

const PAIRS = [
  { canonical: "license-ru.txt", copy: "license_ru.txt" },
  { canonical: "license-en.txt", copy: "license_en.txt" },
];

function normalize(raw) {
  const text = raw.replace(/^﻿/, "").replace(/\r\n?/g, "\n");
  return Buffer.from(BOM + text.replace(/\n/g, "\r\n"), "utf8");
}

function readOrNull(file) {
  try {
    return fs.readFileSync(file);
  } catch (e) {
    return null;
  }
}

function run(check) {
  const problems = [];

  for (const pair of PAIRS) {
    const canonicalFile = path.join(BUILD_DIR, pair.canonical);
    const copyFile = path.join(BUILD_DIR, pair.copy);

    const source = readOrNull(canonicalFile);
    if (!source) {
      problems.push(`нет канонического файла ${pair.canonical}`);
      continue;
    }

    const wanted = normalize(source.toString("utf8"));

    for (const [file, name] of [
      [canonicalFile, pair.canonical],
      [copyFile, pair.copy],
    ]) {
      const current = readOrNull(file);

      if (current && current.equals(wanted)) continue;

      if (check) {
        problems.push(
          current ? `${name} расходится с каноническим текстом` : `нет файла ${name}`
        );
        continue;
      }

      fs.writeFileSync(file, wanted);
      process.stdout.write(`обновлён ${name}\n`);
    }
  }

  return problems;
}

const problems = run(process.argv.includes("--check"));

if (problems.length) {
  process.stderr.write(`Лицензия рассинхронизирована:\n  ${problems.join("\n  ")}\n`);
  process.stderr.write("Выполните: npm run eula:sync\n");
  process.exit(1);
}

module.exports = { PAIRS, normalize };
