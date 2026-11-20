"use strict";

const schema = require("./event-schema");

const CONTRACT_VERSION = 1;

const TYPE_MAP = {
  run_start: "run_started",
  run_restart: "run_restarted",
  run_complete: "run_completed",
  run_abandon: "run_abandoned",
  run_pb: "run_personal_best",
  run_tick: "run_heartbeat",
  map_enter: "map_entered",
  map_leave: "map_left",
};

const TOKEN_RE = /^[0-9a-f]{8,32}$/i;
const MAP_RE = /^[0-9a-z][0-9a-z._-]{0,63}$/i;
const TIMESTAMP_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/;

const MAX_LINE_BYTES = 1024;

/**
 * Разбор одной строки контракта.
 *
 * @returns {{ok: true, record: object} | {ok: false, reason: string}}
 */
function parseLine(line) {
  if (typeof line !== "string") return { ok: false, reason: "not-string" };

  const trimmed = line.trim();
  if (!trimmed) return { ok: false, reason: "empty" };
  if (Buffer.byteLength(trimmed, "utf8") > MAX_LINE_BYTES) return { ok: false, reason: "too-long" };

  let data;
  try {
    data = JSON.parse(trimmed);
  } catch (error) {
    return { ok: false, reason: "not-json" };
  }

  if (!data || typeof data !== "object" || Array.isArray(data)) return { ok: false, reason: "not-object" };
  if (data.v !== CONTRACT_VERSION) return { ok: false, reason: "version" };

  const type = TYPE_MAP[data.type];
  if (!type) return { ok: false, reason: "unknown-type" };

  if (!Number.isInteger(data.seq) || data.seq < 0) return { ok: false, reason: "seq" };

  if (typeof data.t !== "string" || !TIMESTAMP_RE.test(data.t)) return { ok: false, reason: "timestamp" };
  if (typeof data.run !== "string" || !TOKEN_RE.test(data.run)) return { ok: false, reason: "run-token" };

  const record = {
    seq: data.seq,
    occurredAt: data.t,
    token: data.run.toLowerCase(),
    type,
    fields: {},
  };

  if (data.cat !== undefined) {
    if (typeof data.cat !== "string" || !MAP_RE.test(data.cat)) return { ok: false, reason: "category" };
    record.fields.categoryId = data.cat;
  }

  if (data.map !== undefined) {
    if (typeof data.map !== "string" || !MAP_RE.test(data.map)) return { ok: false, reason: "map" };
    record.fields.mapId = data.map;
  }

  if (data.ch !== undefined) {
    if (typeof data.ch !== "string" || !MAP_RE.test(data.ch)) return { ok: false, reason: "chapter" };
    record.fields.chapterId = data.ch;
  }

  if (data.ms !== undefined) {
    if (!schema.checkField("elapsedMilliseconds", data.ms)) return { ok: false, reason: "elapsed" };
    record.fields.elapsedMilliseconds = data.ms;
  }

  if (data.seg !== undefined) {
    if (!schema.checkField("segmentMilliseconds", data.seg)) return { ok: false, reason: "segment" };
    record.fields.segmentMilliseconds = data.seg;
  }

  if (data.rst !== undefined) {
    if (!schema.checkField("restartCount", data.rst)) return { ok: false, reason: "restart-count" };
    record.fields.restartCount = data.rst;
  }

  if (data.pb !== undefined) {
    if (typeof data.pb !== "boolean") return { ok: false, reason: "pb" };
    record.fields.isPersonalBest = data.pb;
  }

  const allowed = ["v", "seq", "t", "type", "run", "cat", "map", "ch", "ms", "seg", "rst", "pb"];
  for (const key of Object.keys(data))
    if (allowed.indexOf(key) === -1) return { ok: false, reason: `unknown-field:${key}` };

  return { ok: true, record };
}

/**
 * Проверка последовательности забега.
 *
 * Лаунчер не должен пересылать в телеметрию заведомо невозможные цепочки:
 * завершение забега, который не начинался, или время, идущее назад.
 * Сервер проверит это ещё раз (клиент недоверенный), но отправлять мусор,
 * зная, что он мусор, — плохой тон и лишний трафик.
 */
class RunSequence {
  constructor() {
    this.runs = new Map();
  }

  accept(record) {
    const state = this.runs.get(record.token);

    if (record.type === "run_started") {
      this.runs.set(record.token, { started: true, lastElapsed: 0, lastSeq: record.seq, finished: false });
      return { ok: true };
    }

    if (!state || !state.started) return { ok: false, reason: "run-not-started" };
    if (state.finished) return { ok: false, reason: "run-already-finished" };

    if (record.seq <= state.lastSeq) return { ok: false, reason: "seq-not-increasing" };

    const elapsed = record.fields.elapsedMilliseconds;
    if (elapsed !== undefined) {
      if (record.type !== "run_restarted" && elapsed < state.lastElapsed)
        return { ok: false, reason: "time-went-backwards" };

      state.lastElapsed = record.type === "run_restarted" ? 0 : elapsed;
    }

    state.lastSeq = record.seq;

    if (record.type === "run_completed" || record.type === "run_abandoned") state.finished = true;

    return { ok: true };
  }

  forget(token) {
    this.runs.delete(token);
  }

  clear() {
    this.runs.clear();
  }
}

/**
 * Токен игры -> настоящий runId (UUID).
 *
 * Игра не умеет генерировать криптостойкие UUID без лишней возни, и это
 * не её работа. Она даёт короткий токен, уникальный в пределах запуска;
 * стабильный случайный идентификатор присваивает лаунчер, у которого
 * есть crypto. Заодно это гарантирует, что runId точно не окажется
 * именем файла .run или чем-то ещё осмысленным.
 */
class RunTokenMap {
  constructor(generateUuid) {
    this.generate = generateUuid;
    this.map = new Map();
    this.limit = 500;
  }

  runIdFor(token) {
    const existing = this.map.get(token);
    if (existing) return existing;

    if (this.map.size >= this.limit) {
      const oldest = this.map.keys().next();
      if (!oldest.done) this.map.delete(oldest.value);
    }

    const runId = this.generate();
    this.map.set(token, runId);
    return runId;
  }

  clear() {
    this.map.clear();
  }
}

module.exports = {
  CONTRACT_VERSION,
  TYPE_MAP,
  MAX_LINE_BYTES,
  parseLine,
  RunSequence,
  RunTokenMap,
};
