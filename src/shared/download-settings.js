"use strict";

const SPEED_LIMITS = [0, 1, 2, 5, 10, 20, 50, 100];

function validSpeedLimit(value) {
  return typeof value === "number" && SPEED_LIMITS.includes(value);
}

function normalizeSpeedLimit(value) {
  return validSpeedLimit(value) ? value : 0;
}

module.exports = { SPEED_LIMITS, validSpeedLimit, normalizeSpeedLimit };
