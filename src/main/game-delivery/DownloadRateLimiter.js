"use strict";

const { performance } = require("perf_hooks");
const { Transform } = require("stream");

function abortError() {
  const error = new Error("Download stopped");
  error.name = "AbortError";
  error.code = "ABORT_ERR";
  return error;
}

// Общая очередь для всех загрузок: параллельные потоки делят один лимит.
// Не накапливаем кредит во время простоя, чтобы начало загрузки не давало
// неограниченный скачок скорости. Изменение лимита будит текущие ожидания.
class DownloadRateLimiter {
  constructor(bytesPerSecond = 0) {
    this.bytesPerSecond = Number.isFinite(bytesPerSecond) ? Math.max(0, bytesPerSecond) : 0;
    this.queue = [];
    this.timer = null;
    this.lastTick = performance.now();
  }

  setLimit(bytesPerSecond) {
    this.tick();
    this.bytesPerSecond = Number.isFinite(bytesPerSecond) ? Math.max(0, bytesPerSecond) : 0;
    this.schedule();
  }

  tick() {
    const now = performance.now();
    let budget = (now - this.lastTick) * this.bytesPerSecond / 1000;
    this.lastTick = now;

    while (this.queue.length && budget >= this.queue[0].remaining) {
      const item = this.queue.shift();
      budget -= item.remaining;
      item.signal.removeEventListener("abort", item.onAbort);
      item.resolve();
    }
    if (this.queue.length) this.queue[0].remaining -= budget;
  }

  schedule() {
    clearTimeout(this.timer);
    this.timer = null;
    if (!this.bytesPerSecond) {
      for (const item of this.queue.splice(0)) {
        item.signal.removeEventListener("abort", item.onAbort);
        item.resolve();
      }
      return;
    }
    if (!this.queue.length) return;
    const ms = this.queue[0].remaining * 1000 / this.bytesPerSecond;
    this.timer = setTimeout(() => {
      this.tick();
      this.schedule();
    }, Math.max(1, Math.min(ms, 1000)));
  }

  acquire(bytes, signal) {
    if (signal.aborted) return Promise.reject(abortError());
    this.tick();
    if (!this.bytesPerSecond) return Promise.resolve();

    return new Promise((resolve, reject) => {
      const item = { remaining: bytes, signal, resolve, reject };
      item.onAbort = () => {
        this.tick();
        const index = this.queue.indexOf(item);
        if (index < 0) return;
        this.queue.splice(index, 1);
        signal.removeEventListener("abort", item.onAbort);
        reject(abortError());
        this.schedule();
      };
      signal.addEventListener("abort", item.onAbort, { once: true });
      this.queue.push(item);
      this.schedule();
    });
  }

  createStream() {
    const controller = new AbortController();
    return new Transform({
      transform: (chunk, encoding, callback) => {
        this.acquire(chunk.length, controller.signal).then(
          () => callback(null, chunk), callback
        );
      },
      destroy(error, callback) {
        controller.abort();
        callback(error);
      },
    });
  }
}

module.exports = DownloadRateLimiter;
