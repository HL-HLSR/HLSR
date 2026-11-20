"use strict";

const fs = require("fs");
const path = require("path");

class TelemetryQueue {
  constructor(options) {
    const opts = options || {};
    if (!opts.file) throw new Error("TelemetryQueue: не задан путь к файлу");

    this.file = opts.file;
    this.maxEvents = opts.maxEvents || 1000;
    this.maxBytes = opts.maxBytes || 1024 * 1024;
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
    this.quarantined = false;
    this.count = null;
    this.generation = 0;
  }

  exists() {
    try {
      return fs.existsSync(this.file);
    } catch (error) {
      return false;
    }
  }

  readAll() {
    if (!this.exists()) return { events: [], broken: 0 };

    let raw;
    try {
      raw = fs.readFileSync(this.file, "utf8");
    } catch (error) {
      this.logger.warn(`[telemetry] очередь не читается: ${error.message}`);
      return { events: [], broken: 0 };
    }

    const events = [];
    let broken = 0;

    for (const line of raw.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;

      try {
        const parsed = JSON.parse(trimmed);
        if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) events.push(parsed);
        else broken++;
      } catch (error) {
        broken++;
      }
    }

    if (broken > 0 && broken >= events.length) {
      this.quarantine("нечитаемое содержимое");
      this.count = 0;
      return { events: [], broken };
    }

    this.count = events.length;
    return { events, broken };
  }

  append(event) {
    if (this.quarantined) return false;

    let line;
    try {
      line = JSON.stringify(event);
    } catch (error) {
      return false;
    }

    if (Buffer.byteLength(line, "utf8") > 8 * 1024) {
      this.logger.warn("[telemetry] событие слишком велико и отброшено");
      return false;
    }

    if (this.count === null) this.readAll();

    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true });
      fs.appendFileSync(this.file, line + "\n", "utf8");
      this.count = (this.count || 0) + 1;
    } catch (error) {
      this.logger.warn(`[telemetry] запись в очередь не удалась: ${error.message}`);
      return false;
    }

    this.enforceLimits();
    return true;
  }

  enforceLimits() {
    let size = 0;
    try {
      size = fs.statSync(this.file).size;
    } catch (error) {
      return;
    }

    if (size <= this.maxBytes && (this.count || 0) <= this.maxEvents) return;

    const { events } = this.readAll();
    if (events.length <= this.maxEvents && size <= this.maxBytes) return;

    let kept = events.slice(Math.max(0, events.length - this.maxEvents));

    let serialized = kept.map((event) => JSON.stringify(event));
    let bytes = serialized.reduce((sum, line) => sum + Buffer.byteLength(line, "utf8") + 1, 0);

    while (bytes > this.maxBytes && serialized.length > 0) {
      bytes -= Buffer.byteLength(serialized[0], "utf8") + 1;
      serialized.shift();
      kept.shift();
    }

    const dropped = events.length - kept.length;
    if (dropped > 0)
      this.logger.warn(`[telemetry] очередь переполнена, отброшено самых старых событий: ${dropped}`);

    this.rewrite(serialized);
  }

  rewrite(lines) {
    if (lines.length === 0 && !this.exists()) {
      this.count = 0;
      return;
    }

    const temp = `${this.file}.${process.pid}.tmp`;

    try {
      fs.writeFileSync(temp, lines.length ? lines.join("\n") + "\n" : "", "utf8");
      fs.renameSync(temp, this.file);
      this.count = lines.length;
    } catch (error) {
      this.logger.warn(`[telemetry] перезапись очереди не удалась: ${error.message}`);
      try {
        fs.rmSync(temp, { force: true });
      } catch (e) {
        /* временный файл — не повод падать */
      }
    }
  }

  peek(maxEvents, maxBytes) {
    const { events } = this.readAll();
    const batch = [];
    let bytes = 2;

    for (const event of events) {
      if (batch.length >= maxEvents) break;

      const size = Buffer.byteLength(JSON.stringify(event), "utf8") + 1;
      if (batch.length > 0 && bytes + size > maxBytes) break;

      bytes += size;
      batch.push(event);
    }

    return batch;
  }

  remove(count) {
    if (count <= 0) return;

    const { events } = this.readAll();
    const kept = events.slice(count);
    this.rewrite(kept.map((event) => JSON.stringify(event)));
  }

  size() {
    return this.readAll().events.length;
  }

  purge() {
    try {
      if (fs.existsSync(this.file)) fs.rmSync(this.file, { force: true });
      this.count = 0;
      this.generation++;
      return true;
    } catch (error) {
      this.logger.warn(`[telemetry] очередь не удалена: ${error.message}`);
      return false;
    }
  }

  quarantine(reason) {
    if (this.quarantined) return;
    this.quarantined = true;

    try {
      const target = `${this.file}.corrupt-${Date.now().toString(36)}`;
      fs.renameSync(this.file, target);
      this.logger.warn(
        `[telemetry] очередь карантинирована (${reason}): ${path.basename(target)}`
      );
    } catch (error) {
      try {
        fs.rmSync(this.file, { force: true });
      } catch (e) {
        /* ничего больше сделать нельзя */
      }
    }

    this.quarantined = false;
  }
}

module.exports = TelemetryQueue;
