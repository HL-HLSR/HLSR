"use strict";

const ACTIONS = {
  play: "play",
  stop: "stop",
  install: "install",
  update: "update",
  pause: "pause",
  resume: "resume",
  retry: "retry",
  repair: "repair",
  explain: "explain",
  none: "none",
};

const UPDATE_OPERATIONS = ["update", "repair-download"];

const { BUSY_STATES: BUSY_DELIVERY_STATES } = require("../../shared/progress-states");

function state(name, action, options) {
  return Object.assign(
    {
      state: name,
      action: action,
      enabled: action !== ACTIONS.none,
      spinner: false,
      progress: false,
    },
    options || {}
  );
}

function fromProcess(process) {
  switch (process.state) {
    case "stopping":
      return state("stopping-game", ACTIONS.none, { spinner: true });
    case "starting":
      return state("starting-game", ACTIONS.none, { spinner: true });
    case "running":
      return state("game-running", ACTIONS.stop);
    default:
      return null;
  }
}

function fromUpdateOperation(delivery) {
  switch (delivery.state) {
    case "downloading":
      return state("downloading-update", ACTIONS.pause, { progress: true });
    case "paused":
      return state("update-paused", ACTIONS.resume, { progress: true });
    case "preparing":
    case "verifying":
      return state("verifying-update", ACTIONS.none, { spinner: true, progress: true });
    case "extracting":
    case "finalizing":
    case "applying-update":
      return state("applying-update", ACTIONS.none, { spinner: true, progress: true });
    case "error":
      return state("update-error", ACTIONS.retry);
    default:
      return null;
  }
}

function fromInstallOperation(delivery, installed) {
  switch (delivery.state) {
    case "downloading":
      return state("downloading", ACTIONS.pause, { progress: true });
    case "paused":
      return state("download-paused", ACTIONS.resume, { progress: true });
    case "preparing":
    case "verifying":
    case "extracting":
    case "finalizing":
      return state("installing", ACTIONS.none, { spinner: true, progress: true });
    case "checking-files":
    case "repairing":
    case "exporting":
    case "importing":
      return state("busy", ACTIONS.none, { spinner: true, progress: true });
    case "error":
      return installed ? null : state("error", ACTIONS.retry);
    default:
      return null;
  }
}

function resolve(input) {
  const options = input || {};
  const delivery = options.delivery || {};
  const process = options.process || { state: "not-running" };
  const update = options.update || null;
  const installed = !!delivery.installedVersion;

  const running = fromProcess(process);
  if (running) return running;

  const busy = BUSY_DELIVERY_STATES.indexOf(delivery.state) !== -1 || delivery.state === "paused";
  const updating = UPDATE_OPERATIONS.indexOf(delivery.operation) !== -1;

  if (busy || delivery.state === "error") {
    const resolved = updating
      ? fromUpdateOperation(delivery)
      : fromInstallOperation(delivery, installed);

    if (resolved) return resolved;
  }

  if (installed && delivery.state === "files-damaged")
    return state("repair-required", ACTIONS.repair, {
      damaged: delivery.integrity ? delivery.integrity.damaged : null,
      reason: delivery.metadataIssue ? delivery.metadataIssue.code : null,
    });

  if (installed && delivery.metadataState === "launcher-update-required")
    return state("update-launcher-required", ACTIONS.explain, {
      reason: delivery.unavailableReason || "BUILD_REQUIRES_NEWER_LAUNCHER",
    });

  if (options.checkingUpdates) return state("checking-updates", ACTIONS.none, { spinner: true });

  if (installed && update && update.updateAvailable)
    return state("update-available", ACTIONS.update, {
      required: !!update.updateRequired,
      availableVersion: update.availableVersion || null,
    });

  if (installed) return state("ready", ACTIONS.play);

  if (delivery.state === "error") return state("error", ACTIONS.retry);

  if (delivery.installable === false)
    return state("unavailable", ACTIONS.explain, {
      reason: delivery.unavailableReason || "UNKNOWN_UNAVAILABLE",
    });

  return state("not-installed", ACTIONS.install);
}

module.exports = {
  ACTIONS,
  BUSY_DELIVERY_STATES,
  UPDATE_OPERATIONS,
  resolve,
};
