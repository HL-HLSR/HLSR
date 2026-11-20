#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const TEST_DIR = path.resolve(__dirname, "..", "test", "delivery");

function collect(dir) {
  const files = [];

  for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, item.name);
    if (item.isDirectory()) files.push(...collect(full));
    else if (item.name.endsWith(".test.js")) files.push(full);
  }

  return files.sort();
}

// test/ исключён из git (.gitignore, решение владельца от 10.08.2026): в клоне без него проверять нечего.
if (!fs.existsSync(TEST_DIR)) {
  process.stdout.write(`Тестов доставки нет в репозитории (test/ исключён из git): ${TEST_DIR}\n`);
  process.exit(0);
}

const files = collect(TEST_DIR);

if (!files.length) {
  process.stderr.write(`Тестов не найдено в ${TEST_DIR}\n`);
  process.exit(1);
}

const args = ["--test"].concat(process.argv.slice(2)).concat(files);
const result = spawnSync(process.execPath, args, { stdio: "inherit", windowsHide: true });

process.exit(result.status == null ? 1 : result.status);
