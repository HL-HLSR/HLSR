"use strict";

// Минимальный клиент Chrome DevTools Protocol поверх ws: хватает на
// Runtime.evaluate, ввод мышью/клавиатурой и скриншоты. Тот же клиент
// подключается и к инспектору главного процесса (--inspect), там протокол
// совпадает по Runtime.*.

const http = require("http");
const WebSocket = require("ws");

function getJson(url, timeoutMs = 3000) {
  return new Promise((resolve, reject) => {
    const req = http.get(url, { timeout: timeoutMs }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on("timeout", () => req.destroy(new Error(`timeout ${url}`)));
    req.on("error", reject);
  });
}

async function waitForTarget(port, predicate, timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const targets = await getJson(`http://127.0.0.1:${port}/json/list`);
      const target = targets.find(predicate);
      if (target && target.webSocketDebuggerUrl) return target;
    } catch (error) {
      lastError = error;
    }
    await delay(300);
  }
  throw new Error(`CDP: цель на порту ${port} не появилась${lastError ? `: ${lastError.message}` : ""}`);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

class CdpSession {
  constructor(url) {
    this.url = url;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
    this.ws = null;
  }

  connect() {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url, { perMessageDeflate: false, maxPayload: 256 * 1024 * 1024 });
      ws.once("open", () => {
        this.ws = ws;
        resolve(this);
      });
      ws.once("error", reject);
      ws.on("message", (raw) => this.onMessage(raw));
      ws.on("close", () => {
        for (const { reject: fail } of this.pending.values()) fail(new Error("CDP: соединение закрыто"));
        this.pending.clear();
      });
    });
  }

  onMessage(raw) {
    const message = JSON.parse(String(raw));
    if (message.id && this.pending.has(message.id)) {
      const { resolve, reject } = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) reject(new Error(`CDP ${message.error.code}: ${message.error.message}`));
      else resolve(message.result);
      return;
    }
    const handlers = this.listeners.get(message.method);
    if (handlers) for (const handler of handlers) handler(message.params);
  }

  on(method, handler) {
    if (!this.listeners.has(method)) this.listeners.set(method, new Set());
    this.listeners.get(method).add(handler);
  }

  send(method, params = {}, timeoutMs = 30000) {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return Promise.reject(new Error("CDP: нет соединения"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP: ${method} не ответил за ${timeoutMs} мс`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  // Выполняет выражение и возвращает значение как JSON. Исключение страницы
  // превращается в ошибку с текстом, а не в молчаливый undefined.
  async evaluate(expression, { awaitPromise = true, timeoutMs = 30000 } = {}) {
    const result = await this.send(
      "Runtime.evaluate",
      { expression, awaitPromise, returnByValue: true, includeCommandLineAPI: true },
      timeoutMs
    );
    if (result.exceptionDetails) {
      const details = result.exceptionDetails;
      const text = (details.exception && (details.exception.description || details.exception.value)) || details.text;
      throw new Error(`evaluate: ${text}`);
    }
    return result.result ? result.result.value : undefined;
  }

  close() {
    if (this.ws) this.ws.close();
  }
}

module.exports = { CdpSession, getJson, waitForTarget, delay };
