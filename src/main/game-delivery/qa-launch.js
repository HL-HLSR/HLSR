"use strict";

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

const qaMode = require("./qa-mode");
const { assertInside } = require("./paths");
const { DeliveryError } = require("./errors");

const EXE_FLAG = "HLSR_GAME_DELIVERY_QA_LAUNCH_EXE";
const ARGS_FLAG = "HLSR_GAME_DELIVERY_QA_LAUNCH_ARGS";
const DELAY_FLAG = "HLSR_GAME_DELIVERY_QA_LAUNCH_DELAY";

function isEnabled(env) {
  const source = env || process.env;
  if (qaMode.PRODUCTION_BUILD) return false;
  if (!qaMode.isEnabled(source)) return false;
  return !!source[EXE_FLAG];
}

function describe(env) {
  const source = env || process.env;

  return {
    enabled: isEnabled(source),
    executable: source[EXE_FLAG] || null,
    args: source[ARGS_FLAG] || null,
  };
}

function parseArgs(raw) {
  if (!raw) return [];

  const matches = String(raw).match(/"[^"]*"|\S+/g) || [];
  return matches.map((item) => item.replace(/^"|"$/g, ""));
}

function startupDelay(env) {
  const value = Number((env || process.env)[DELAY_FLAG]);
  return Number.isFinite(value) && value > 0 ? Math.min(value, 10000) : 0;
}

function launch(installPath, env) {
  const source = env || process.env;

  if (!isEnabled(source))
    throw new DeliveryError("PROVIDER_UNSUPPORTED", "Тестовый запуск доступен только в QA-режиме");

  const relative = source[EXE_FLAG];

  if (path.isAbsolute(relative))
    throw new DeliveryError(
      "PATH_INVALID",
      "Тестовый исполняемый файл задаётся путём относительно папки игры"
    );

  const executable = assertInside(
    installPath,
    path.join(installPath, relative.replace(/\//g, path.sep)),
    "Тестовый исполняемый файл"
  );

  if (!fs.existsSync(executable))
    throw new DeliveryError("REQUIRED_FILE_MISSING", `Тестовый процесс не найден: ${executable}`);

  const args = parseArgs(source[ARGS_FLAG]);

  const child = spawn(executable, args, {
    cwd: installPath,
    windowsHide: true,
    detached: true,
    stdio: "ignore",
  });

  return {
    child,
    pid: child.pid,
    executablePath: executable,
    startedAt: new Date().toISOString(),
  };
}

module.exports = {
  EXE_FLAG,
  ARGS_FLAG,
  DELAY_FLAG,
  isEnabled,
  describe,
  parseArgs,
  startupDelay,
  launch,
};
