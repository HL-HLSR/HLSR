"use strict";

const { STATES, CODES } = require("./constants");

const STATE_LIST = Object.keys(STATES).map((k) => STATES[k]);
const CODE_LIST = Object.keys(CODES).map((k) => CODES[k]);

const RECONNECT_STATES = [STATES.RETRY_WAIT, STATES.UNAVAILABLE, STATES.DISCONNECTED];

const DISABLED = {
  enabled: false,
  state: STATES.DISABLED,
  connected: false,
  lastStableCode: CODES.DISABLED,
  reconnectPending: false,
  lastConnectedAt: null,
  gameBridge: null,
};

const BRIDGE_KEYS = ["listening", "connected", "rejected", "lifecycle", "hasMap"];
const BRIDGE_LIFECYCLES = ["menu", "playing", "exited"];

function normalizeBridge(raw) {
  if (!raw || typeof raw !== "object") return null;
  const rejected = Number(raw.rejected);
  return {
    listening: raw.listening === true,
    connected: raw.connected === true,
    rejected: Number.isFinite(rejected) && rejected >= 0 ? Math.floor(rejected) : 0,
    lifecycle: BRIDGE_LIFECYCLES.indexOf(raw.lifecycle) !== -1 ? raw.lifecycle : null,
    hasMap: raw.hasMap === true,
  };
}

function normalize(raw) {
  if (!raw || typeof raw !== "object") return Object.assign({}, DISABLED);

  const bridge = normalizeBridge(raw.gameBridge);

  const enabled = raw.enabled === true;
  if (!enabled) return Object.assign({}, DISABLED, { gameBridge: bridge });

  const state = STATE_LIST.indexOf(raw.state) !== -1 ? raw.state : STATES.DISCONNECTED;

  const code = raw.lastCode || raw.lastStableCode || raw.code;
  const lastStableCode = CODE_LIST.indexOf(code) !== -1 ? code : null;

  const at = Number(raw.lastConnectedAt);
  const lastConnectedAt = Number.isFinite(at) && at > 0 ? Math.floor(at) : null;

  return {
    enabled: true,
    state,
    connected: state === STATES.CONNECTED,
    lastStableCode,
    reconnectPending: RECONNECT_STATES.indexOf(state) !== -1,
    lastConnectedAt,
    gameBridge: bridge,
  };
}

const ALLOWED_KEYS = Object.keys(DISABLED);

module.exports = {
  DISABLED,
  ALLOWED_KEYS,
  BRIDGE_KEYS,
  RECONNECT_STATES,
  normalize,
  normalizeBridge,
};
