"use strict";

const fs = require("fs");
const path = require("path");

const FILE = "settings.json";

const KEY = "gpuAcceleration";

const DEFAULT_ENABLED = true;
const runtimes = new WeakMap();

function readEnabled(userDataDir) {
  try {
    const raw = fs.readFileSync(path.join(userDataDir, FILE), "utf8");
    const data = JSON.parse(raw.replace(/^\uFEFF/, ""));

    if (!data || typeof data !== "object" || !data.config) return DEFAULT_ENABLED;

    return typeof data.config[KEY] === "boolean" ? data.config[KEY] : DEFAULT_ENABLED;
  } catch (e) {
    return DEFAULT_ENABLED;
  }
}

function apply(app) {
  const enabled = readEnabled(app.getPath("userData"));
  const runtime = { appliedEnabled: enabled, ready: false };
  runtimes.set(app, runtime);
  app.once("gpu-info-update", () => { runtime.ready = true; });

  if (!enabled) {
    app.disableHardwareAcceleration();
    app.commandLine.appendSwitch("disable-gpu-compositing");
  }

  return enabled;
}

function waitForInfo(app, runtime, timeoutMs) {
  if (runtime.ready) return Promise.resolve(true);
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      app.removeListener("gpu-info-update", done);
      runtime.ready = true;
      resolve(true);
    };
    const timer = setTimeout(() => {
      app.removeListener("gpu-info-update", done);
      resolve(false);
    }, timeoutMs);
    app.once("gpu-info-update", done);
  });
}

async function snapshot(app, options = {}) {
  const runtime = runtimes.get(app);
  const state = {
    configuredEnabled: readEnabled(app.getPath("userData")),
    appliedEnabled: runtime ? runtime.appliedEnabled : null,
    available: false,
    hardwareEnabled: null,
    compositing: null,
    softwareRenderer: false,
    device: "",
    driver: "",
  };
  const timeoutMs = options.timeoutMs || 3000;
  if (!runtime || !await waitForInfo(app, runtime, timeoutMs)) return state;
  try {
    state.hardwareEnabled = app.isHardwareAccelerationEnabled();
    state.compositing = app.getGPUFeatureStatus().gpu_compositing || null;
    state.available = typeof state.hardwareEnabled === "boolean" && !!state.compositing;
  } catch (error) {
    return state;
  }

  let timer;
  try {
    const info = await Promise.race([
      app.getGPUInfo("complete"),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("GPU info timed out")), timeoutMs);
      }),
    ]);
    const auxiliary = (info && info.auxAttributes) || {};
    const devices = (info && info.gpuDevice) || [];
    const device = devices.find((item) => item.active) || devices[0] || {};
    const renderer = String(auxiliary.glRenderer || "");
    state.softwareRenderer = /swiftshader|llvmpipe|softpipe|software rasterizer|microsoft basic render/i.test(renderer);
    const vendors = { 4098: "AMD", 4318: "NVIDIA", 32902: "Intel" };
    state.device = String(device.deviceString || vendors[device.vendorId] || "").slice(0, 128);
    state.driver = String(device.driverVersion || auxiliary.driverVersion || "").slice(0, 80);
  } catch (error) {
    // Даже без сведений о драйвере доступны фактические флаги Chromium.
  } finally {
    clearTimeout(timer);
  }
  return state;
}

module.exports = { apply, readEnabled, snapshot, KEY, DEFAULT_ENABLED };
