"use strict";

const fs = require("fs");
const path = require("path");

const redact = require("../shared/diagnostics/redact");

const MAX_BYTES = 512 * 1024;

class UpdaterLog {
  constructor(options) {
    const opts = options || {};
    this.file = opts.file || null;
    this.now = opts.now || (() => new Date());
    this.echo = opts.echo !== false;
  }

  rotateIfNeeded() {
    if (!this.file) return;

    try {
      if (!fs.existsSync(this.file)) return;
      if (fs.statSync(this.file).size < MAX_BYTES) return;

      fs.renameSync(this.file, `${this.file}.1`);
    } catch (e) {
      /* ротация не должна ломать обновление */
    }
  }

  write(level, message) {
    const safe = redact.redactText(String(message == null ? "" : message)).slice(0, 2000);
    const line = `[${this.now().toISOString()}] [${level}] ${safe}`;

    if (this.echo) {
      console.log(line);
    }

    if (!this.file) return;

    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true });
      this.rotateIfNeeded();
      fs.appendFileSync(this.file, line + "\n", "utf8");
    } catch (e) {
      /* лог не критичен: обновление важнее записи о нём */
    }
  }

  info(message) {
    this.write("info", message);
  }

  warn(message) {
    this.write("warn", message);
  }

  error(message) {
    this.write("error", message);
  }
}

module.exports = UpdaterLog;
module.exports.MAX_BYTES = MAX_BYTES;
