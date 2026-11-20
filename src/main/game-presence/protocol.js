"use strict";

const mapAssets = require("./map-assets");

const PROTOCOL_VERSION = 1;

const MAX_LINE_BYTES = 2048;

const TOKEN_RE = /^[0-9a-f]{64}$/;
const SESSION_RE = /^[0-9a-f]{8,64}$/;

const LIFECYCLES = ["menu", "playing", "exited"];

const EMOTE_NAME_RE = /^[A-Za-z0-9_()!?:-]{1,47}$/;

const EMOTE_NS_TWITCH = "tw:";
const TWITCH_EMOTE_ID_RE = /^[A-Za-z0-9_]{1,44}$/;
const VIDEO_ID_RE = /^[A-Za-z0-9_-]{5,32}$/;
const ROOM_ID_RE = /^[0-9]{1,20}$/;

const EMOTE_SOURCE_RE = /^(?:[0-9]{1,20}|twitch:[0-9]{1,20}|(?:set|user):[0-9A-Za-z]{1,32})$/;

const PLATFORMS = ["twitch", "youtube"];

const HELLO_FIELDS = ["v", "type", "token", "session"];
const CHAT_SUBSCRIBE_FIELDS = ["v", "type", "session", "platform", "id"];
const CHAT_UNSUBSCRIBE_FIELDS = ["v", "type", "session"];
const EMOTE_SET_FIELDS = ["v", "type", "session", "roomId"];
const EMOTE_RESOLVE_FIELDS = ["v", "type", "session", "name", "px"];
const EMOTE_PX_MIN = 8;
const EMOTE_PX_MAX = 128;
const STATE_FIELDS = [
  "v",
  "type",
  "session",
  "seq",
  "lifecycle",
  "mapName",
  "fps",
  "timerRunning",
  "timerCompleted",
  "timerElapsedMilliseconds",
];

function fail(reason) {
  return { ok: false, reason };
}

function keysWithin(object, allowed) {
  for (const key of Object.keys(object)) if (allowed.indexOf(key) === -1) return key;
  return null;
}

function parseLine(line) {
  if (typeof line !== "string") return fail("NOT_A_STRING");
  if (Buffer.byteLength(line, "utf8") > MAX_LINE_BYTES) return fail("TOO_LONG");

  let raw;
  try {
    raw = JSON.parse(line);
  } catch (e) {
    return fail("NOT_JSON");
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return fail("NOT_AN_OBJECT");
  if (raw.v !== PROTOCOL_VERSION) return fail("BAD_VERSION");

  if (raw.type === "hello") return parseHello(raw);
  if (raw.type === "state") return parseState(raw);
  if (raw.type === "chat.subscribe") return parseChatSubscribe(raw);
  if (raw.type === "chat.unsubscribe") return parseChatUnsubscribe(raw);
  if (raw.type === "emote.set") return parseEmoteSet(raw);
  if (raw.type === "emote.resolve") return parseEmoteResolve(raw);
  return fail("UNKNOWN_TYPE");
}

function parseChatSubscribe(raw) {
  const extra = keysWithin(raw, CHAT_SUBSCRIBE_FIELDS);
  if (extra) return fail(`UNKNOWN_FIELD:${extra}`);
  if (typeof raw.session !== "string" || !SESSION_RE.test(raw.session)) return fail("BAD_SESSION");
  if (PLATFORMS.indexOf(raw.platform) === -1) return fail("BAD_PLATFORM");
  if (typeof raw.id !== "string") return fail("BAD_ID");

  if (raw.platform === "youtube" && !VIDEO_ID_RE.test(raw.id)) return fail("BAD_ID");
  if (raw.platform === "twitch" && !ROOM_ID_RE.test(raw.id)) return fail("BAD_ID");

  return { ok: true, message: { type: "chat.subscribe", session: raw.session, platform: raw.platform, id: raw.id } };
}

function parseChatUnsubscribe(raw) {
  const extra = keysWithin(raw, CHAT_UNSUBSCRIBE_FIELDS);
  if (extra) return fail(`UNKNOWN_FIELD:${extra}`);
  if (typeof raw.session !== "string" || !SESSION_RE.test(raw.session)) return fail("BAD_SESSION");
  return { ok: true, message: { type: "chat.unsubscribe", session: raw.session } };
}

function parseEmoteSet(raw) {
  const extra = keysWithin(raw, EMOTE_SET_FIELDS);
  if (extra) return fail(`UNKNOWN_FIELD:${extra}`);
  if (typeof raw.session !== "string" || !SESSION_RE.test(raw.session)) return fail("BAD_SESSION");
  if (typeof raw.roomId !== "string" || !EMOTE_SOURCE_RE.test(raw.roomId)) return fail("BAD_ROOM_ID");
  return { ok: true, message: { type: "emote.set", session: raw.session, roomId: raw.roomId } };
}

function parseEmoteResolve(raw) {
  const extra = keysWithin(raw, EMOTE_RESOLVE_FIELDS);
  if (extra) return fail(`UNKNOWN_FIELD:${extra}`);
  if (typeof raw.session !== "string" || !SESSION_RE.test(raw.session)) return fail("BAD_SESSION");
  if (typeof raw.name !== "string" || !EMOTE_NAME_RE.test(raw.name)) return fail("BAD_EMOTE_NAME");

  let px = null;
  if (raw.px !== undefined) {
    if (!Number.isInteger(raw.px) || raw.px < EMOTE_PX_MIN || raw.px > EMOTE_PX_MAX) return fail("BAD_EMOTE_PX");
    px = raw.px;
  }

  return { ok: true, message: { type: "emote.resolve", session: raw.session, name: raw.name, px } };
}

function parseHello(raw) {
  const extra = keysWithin(raw, HELLO_FIELDS);
  if (extra) return fail(`UNKNOWN_FIELD:${extra}`);
  if (typeof raw.token !== "string" || !TOKEN_RE.test(raw.token)) return fail("BAD_TOKEN");
  if (typeof raw.session !== "string" || !SESSION_RE.test(raw.session)) return fail("BAD_SESSION");
  return { ok: true, message: { type: "hello", token: raw.token, session: raw.session } };
}

function parseState(raw) {
  const extra = keysWithin(raw, STATE_FIELDS);
  if (extra) return fail(`UNKNOWN_FIELD:${extra}`);

  if (typeof raw.session !== "string" || !SESSION_RE.test(raw.session)) return fail("BAD_SESSION");
  if (!Number.isInteger(raw.seq) || raw.seq < 0) return fail("BAD_SEQ");
  if (LIFECYCLES.indexOf(raw.lifecycle) === -1) return fail("BAD_LIFECYCLE");

  if (typeof raw.timerRunning !== "boolean") return fail("BAD_TIMER_RUNNING");
  if (typeof raw.timerCompleted !== "boolean") return fail("BAD_TIMER_COMPLETED");

  let mapName = null;
  if (raw.mapName !== null && raw.mapName !== undefined) {
    mapName = mapAssets.normalizeMapName(raw.mapName);
    if (mapName === null) return fail("BAD_MAP_NAME");
  }

  let fps = null;
  if (raw.fps !== null && raw.fps !== undefined) {
    fps = mapAssets.normalizeFps(raw.fps);
    if (fps === null) return fail("BAD_FPS");
  }

  let elapsedMs = null;
  if (raw.timerElapsedMilliseconds !== null && raw.timerElapsedMilliseconds !== undefined) {
    elapsedMs = mapAssets.normalizeElapsedMs(raw.timerElapsedMilliseconds);
    if (elapsedMs === null) return fail("BAD_ELAPSED");
  }

  if (raw.timerRunning && elapsedMs === null) return fail("RUNNING_WITHOUT_ELAPSED");

  if (raw.lifecycle === "playing" && mapName === null) return fail("PLAYING_WITHOUT_MAP");
  if (raw.lifecycle !== "playing" && mapName !== null) return fail("MAP_OUTSIDE_PLAYING");

  return {
    ok: true,
    message: {
      type: "state",
      session: raw.session,
      seq: raw.seq,
      lifecycle: raw.lifecycle,
      mapName,
      fps,
      timerRunning: raw.timerRunning,
      timerCompleted: raw.timerCompleted,
      elapsedMs,
    },
  };
}

function encodeDiscordAck(connected) {
  return JSON.stringify({ v: PROTOCOL_VERSION, type: "discord", connected: connected === true }) + "\n";
}

function sliceSafe(value, max) {
  const s = String(value == null ? "" : value);
  if (s.length <= max) return s;

  let cut = max;
  const last = s.charCodeAt(cut - 1);
  if (last >= 0xd800 && last <= 0xdbff) cut--;
  return s.slice(0, cut);
}

function encodeChatLine(platform, nick, text, badge1, badge2) {
  return (
    JSON.stringify({
      v: PROTOCOL_VERSION,
      type: "chat.line",
      platform: sliceSafe(platform, 16),
      badge1: sliceSafe(badge1 || "", 31),
      badge2: sliceSafe(badge2 || "", 31),
      nick: sliceSafe(nick, 32),
      text: sliceSafe(text, 240),
    }) + "\n"
  );
}

function encodeEmoteBegin(name, wide, tall, frames, frameMs) {
  return (
    JSON.stringify({
      v: PROTOCOL_VERSION,
      type: "emote.begin",
      name: String(name).slice(0, 47),
      wide: wide | 0,
      tall: tall | 0,
      frames: frames | 0,
      frameMs: frameMs | 0,
    }) + "\n"
  );
}

function encodeEmoteFrame(name, index, rgbaBase64) {
  return (
    JSON.stringify({
      v: PROTOCOL_VERSION,
      type: "emote.frame",
      name: String(name).slice(0, 47),
      index: index | 0,
      rgba: rgbaBase64,
    }) + "\n"
  );
}

function encodeEmoteReady(name, wide, tall, rgbaBase64) {
  return (
    JSON.stringify({
      v: PROTOCOL_VERSION,
      type: "emote.ready",
      name: String(name).slice(0, 47),
      wide: wide | 0,
      tall: tall | 0,
      rgba: rgbaBase64,
    }) + "\n"
  );
}

function encodeBadgeReady(name, wide, tall, rgbaBase64) {
  return (
    JSON.stringify({
      v: PROTOCOL_VERSION,
      type: "badge.ready",
      name: String(name).slice(0, 31),
      wide: wide | 0,
      tall: tall | 0,
      rgba: rgbaBase64,
    }) + "\n"
  );
}

function encodeEmoteMiss(name) {
  return JSON.stringify({ v: PROTOCOL_VERSION, type: "emote.miss", name: sliceSafe(name, 47) }) + "\n";
}

module.exports = {
  EMOTE_NS_TWITCH,
  TWITCH_EMOTE_ID_RE,
  encodeBadgeReady,
  PROTOCOL_VERSION,
  MAX_LINE_BYTES,
  LIFECYCLES,
  PLATFORMS,
  HELLO_FIELDS,
  STATE_FIELDS,
  CHAT_SUBSCRIBE_FIELDS,
  CHAT_UNSUBSCRIBE_FIELDS,
  EMOTE_SET_FIELDS,
  EMOTE_RESOLVE_FIELDS,
  EMOTE_PX_MIN,
  EMOTE_PX_MAX,
  TOKEN_RE,
  SESSION_RE,
  EMOTE_NAME_RE,
  VIDEO_ID_RE,
  ROOM_ID_RE,
  EMOTE_SOURCE_RE,
  parseLine,
  encodeDiscordAck,
  encodeChatLine,
  encodeEmoteReady,
  encodeEmoteBegin,
  encodeEmoteFrame,
  encodeEmoteMiss,
};
