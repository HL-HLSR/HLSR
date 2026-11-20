#!/usr/bin/env node
"use strict";

// `npm run build` и `npm run build:dir` собирают лаунчер под ту ОС, на
// которой запущены: build -> build:win | build:linux,
// build:dir -> build:win:dir | build:linux:dir.

const { spawnSync } = require("child_process");

const PLATFORMS = { win32: "win", linux: "linux" };

const base = process.argv[2];
const platform = PLATFORMS[process.platform];

if (!base || !platform) {
  process.stderr.write(
    `Сборка под ${process.platform} не поддерживается; используйте build:win или build:linux\n`
  );
  process.exit(2);
}

const script = base === "build" ? `build:${platform}` : `build:${platform}:${base.replace(/^build:/, "")}`;
const npmCli = process.env.npm_execpath;

if (!npmCli) {
  process.stderr.write(`Запустите через npm: npm run ${base}\n`);
  process.exit(2);
}

const result = spawnSync(process.execPath, [npmCli, "run", script], { stdio: "inherit" });
process.exit(result.status == null ? 1 : result.status);
