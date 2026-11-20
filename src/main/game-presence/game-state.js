"use strict";

const mapAssets = require("./map-assets");

const ANCHOR_TOLERANCE_MS = 1500;

const FPS_MIN_INTERVAL_MS = 15000;

function emptyState() {
  return {
    active: false,
    session: null,
    lastSeq: -1,
    lifecycle: null,
    map: null,
    mapKnown: false,
    fps: null,
    running: false,
    runStartedAtMs: null,
    finalTime: false,
    runSeenOnMap: null,
  };
}

function sameState(a, b) {
  if (!a || !b) return a === b;
  return (
    a.active === b.active &&
    a.lifecycle === b.lifecycle &&
    a.map === b.map &&
    a.mapKnown === b.mapKnown &&
    a.fps === b.fps &&
    a.running === b.running &&
    a.runStartedAtMs === b.runStartedAtMs &&
    a.finalTime === b.finalTime
  );
}

function sameExceptFps(a, b) {
  if (!a || !b) return a === b;
  return (
    a.active === b.active &&
    a.lifecycle === b.lifecycle &&
    a.map === b.map &&
    a.mapKnown === b.mapKnown &&
    a.running === b.running &&
    a.runStartedAtMs === b.runStartedAtMs &&
    a.finalTime === b.finalTime
  );
}

function apply(previous, message, nowMs) {
  const prior = previous || emptyState();

  if (!message || message.type !== "state") return { state: prior, changed: false, ignored: "NOT_STATE" };

  const newSession = prior.session !== message.session;
  const base = newSession ? emptyState() : prior;

  if (!newSession && message.seq <= base.lastSeq)
    return { state: prior, changed: false, ignored: "STALE_SEQ" };

  const next = Object.assign({}, base, {
    session: message.session,
    lastSeq: message.seq,
    lifecycle: message.lifecycle,
  });

  if (message.lifecycle === "exited") {
    const gone = emptyState();
    gone.session = message.session;
    gone.lastSeq = message.seq;
    gone.lifecycle = "exited";
    return { state: gone, changed: !sameState(prior, gone), ignored: null };
  }

  next.active = true;
  next.fps = message.fps;

  if (message.lifecycle === "menu") {
    next.map = null;
    next.mapKnown = false;
    next.running = false;
    next.runStartedAtMs = null;
    next.finalTime = false;
    return { state: next, changed: !sameState(prior, next), ignored: null };
  }

  const mapChanged = base.map !== message.mapName;
  next.map = message.mapName;
  next.mapKnown = mapAssets.isKnownMap(message.mapName);

  if (mapChanged) {
    next.runStartedAtMs = null;
    next.runSeenOnMap = null;
  }

  next.running = message.timerRunning;

  if (message.timerRunning) {
    next.runSeenOnMap = message.mapName;
    const candidate = nowMs - message.elapsedMs;
    const anchor = mapChanged ? null : base.runStartedAtMs;
    const keep =
      base.running && anchor !== null && Math.abs(candidate - anchor) <= ANCHOR_TOLERANCE_MS;
    next.runStartedAtMs = keep ? anchor : candidate;
    next.finalTime = false;
  } else {
    next.runStartedAtMs = null;

    const wasReset = message.elapsedMs === 0;

    next.finalTime =
      message.timerCompleted === true &&
      !wasReset &&
      next.runSeenOnMap === next.map &&
      next.map === mapAssets.FINAL_TIME_MAP;
  }

  return { state: next, changed: !sameState(prior, next), ignored: null };
}

function shouldPublish(previous, next, lastFpsPublishAtMs, nowMs) {
  if (!previous) return true;
  if (sameState(previous, next)) return false;
  if (!sameExceptFps(previous, next)) return true;
  if (lastFpsPublishAtMs === null || lastFpsPublishAtMs === undefined) return true;
  return nowMs - lastFpsPublishAtMs >= FPS_MIN_INTERVAL_MS;
}

module.exports = {
  ANCHOR_TOLERANCE_MS,
  FPS_MIN_INTERVAL_MS,
  emptyState,
  sameState,
  sameExceptFps,
  apply,
  shouldPublish,
};
