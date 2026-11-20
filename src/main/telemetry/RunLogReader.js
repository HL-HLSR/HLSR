"use strict";

const fs = require("fs");
const path = require("path");

const { assertInside } = require("../game-delivery/paths");
const contract = require("./run-contract");

const RELATIVE_PATH = "ghostingmod/hlsr/run-events.jsonl";

const MAX_READ_BYTES = 256 * 1024;
const POLL_MS = 5000;

class RunLogReader {
  constructor(options) {
    const opts = options || {};

    this.telemetry = opts.telemetry;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.pollMs = opts.pollMs || POLL_MS;
    this.generateUuid = opts.generateUuid;

    this.timer = null;
    this.gameId = null;
    this.file = null;
    this.offset = 0;
    this.sequence = new contract.RunSequence();
    this.tokens = new contract.RunTokenMap(this.generateUuid);
    this.stats = { parsed: 0, rejected: 0, forwarded: 0 };
  }

  resolve(installPath) {
    if (typeof installPath !== "string" || !installPath) return null;

    let target;
    try {
      target = assertInside(
        installPath,
        path.join(installPath, RELATIVE_PATH.replace(/\//g, path.sep)),
        "Файл событий забега"
      );
    } catch (error) {
      return null;
    }

    let stat;
    try {
      stat = fs.lstatSync(target);
    } catch (error) {
      return null;
    }

    if (stat.isSymbolicLink() || !stat.isFile()) return null;

    try {
      const realRoot = fs.realpathSync(installPath);
      const realTarget = fs.realpathSync(target);
      const prefix = realRoot.endsWith(path.sep) ? realRoot : realRoot + path.sep;
      if (!realTarget.toLowerCase().startsWith(prefix.toLowerCase())) return null;
    } catch (error) {
      return null;
    }

    return target;
  }

  attach(gameId, installPath) {
    this.detach();

    const file = this.resolve(installPath);
    if (!file) return false;

    this.gameId = gameId;
    this.file = file;
    try {
      this.offset = fs.statSync(file).size;
    } catch (error) {
      this.offset = 0;
    }

    this.sequence.clear();
    this.tokens.clear();

    this.timer = setInterval(() => this.poll(), this.pollMs);
    if (typeof this.timer.unref === "function") this.timer.unref();

    this.logger.info("[telemetry] приём событий забега активен");
    return true;
  }

  detach() {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }

    if (this.file) {
      try {
        this.poll();
      } catch (error) {
        /* игра уже закрыта — читать нечего */
      }
    }

    this.gameId = null;
    this.file = null;
    this.offset = 0;
    this.sequence.clear();
    this.tokens.clear();
  }

  poll() {
    if (!this.file || !this.gameId || !this.telemetry) return;
    if (!this.telemetry.enabled()) return;

    let size;
    try {
      size = fs.statSync(this.file).size;
    } catch (error) {
      return;
    }

    if (size < this.offset) this.offset = 0;
    if (size === this.offset) return;

    const length = Math.min(size - this.offset, MAX_READ_BYTES);
    let chunk;

    let handle;
    try {
      handle = fs.openSync(this.file, "r");
      const buffer = Buffer.alloc(length);
      const read = fs.readSync(handle, buffer, 0, length, this.offset);
      chunk = buffer.slice(0, read).toString("utf8");
    } catch (error) {
      return;
    } finally {
      if (handle !== undefined) {
        try {
          fs.closeSync(handle);
        } catch (e) {
          /* дескриптор уже закрыт */
        }
      }
    }

    const lastNewline = chunk.lastIndexOf("\n");
    if (lastNewline === -1) return;

    const complete = chunk.slice(0, lastNewline);
    this.offset += Buffer.byteLength(complete, "utf8") + 1;

    for (const line of complete.split("\n")) this.handleLine(line);
  }

  handleLine(line) {
    const parsed = contract.parseLine(line);
    if (!parsed.ok) {
      if (parsed.reason !== "empty") this.stats.rejected++;
      return;
    }

    this.stats.parsed++;

    const accepted = this.sequence.accept(parsed.record);
    if (!accepted.ok) {
      this.stats.rejected++;
      return;
    }

    const meta = this.telemetry.gameSessionMeta(this.gameId);
    const sessionId = this.telemetry.gameSessionId(this.gameId);
    if (!meta || !sessionId) return;

    const fields = Object.assign(
      {
        sessionId,
        gameId: this.gameId,
        gameVersion: meta.gameVersion,
        buildId: meta.buildId,
        runId: this.tokens.runIdFor(parsed.record.token),
      },
      parsed.record.fields,
      { occurredAt: parsed.record.occurredAt }
    );

    if (this.telemetry.emit(parsed.record.type, fields)) this.stats.forwarded++;
  }

  snapshot() {
    return {
      active: this.timer !== null,
      fileFound: this.file !== null,
      parsed: this.stats.parsed,
      rejected: this.stats.rejected,
      forwarded: this.stats.forwarded,
    };
  }
}

module.exports = RunLogReader;
module.exports.RELATIVE_PATH = RELATIVE_PATH;
module.exports.MAX_READ_BYTES = MAX_READ_BYTES;
