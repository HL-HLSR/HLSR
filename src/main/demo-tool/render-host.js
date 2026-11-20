"use strict";

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");
const { EventEmitter } = require("events");

const { DemoError } = require("./errors");
const { powershellPath } = require("../windows-tools");

// Помощник рендера (resources/demo-render/render-host.ps1): скрытый рабочий
// стол, объект задания и глушение звука для процесса игры. Протокол —
// строки JSON в stdin/stdout, см. шапку скрипта.

const SCRIPT = path.join("demo-render", "render-host.ps1");
const READY_TIMEOUT_MS = 60000;
// Обычно 1–5 с (закрытие и беззвучный перезапуск-сброс mute); сброс ждёт сессий до 30 с.
const STOP_TIMEOUT_MS = 60000;
// Окружение игры: без моста Discord (HLSR_PRESENCE_PIPE и прочие HLSR_*),
// переменных Slint/winit/Electron/Node и Steam-переменных запуска из Steam.
const STRIPPED_ENV = /^(HLSR_|SLINT_|WINIT_|ELECTRON_|NODE_OPTIONS$|SteamAppId$|SteamGameId$|SteamOverlayGameId$|SteamAppUser$)/i;

// В сборке скрипт лежит в resources/demo-render (extraResources), из
// исходников — в resources/ репозитория (из бандла dist/electron и из src/main/demo-tool).
function scriptPath(resourcesPath) {
  const candidates = [
    resourcesPath ? path.join(resourcesPath, SCRIPT) : null,
    path.resolve(__dirname, "..", "..", "resources", SCRIPT),
    path.resolve(__dirname, "..", "..", "..", "resources", SCRIPT),
  ].filter(Boolean);
  return candidates.find((file) => fs.existsSync(file)) || null;
}

function childEnv(env) {
  const out = {};
  for (const [key, value] of Object.entries(env || process.env)) if (!STRIPPED_ENV.test(key)) out[key] = value;
  return out;
}

class RenderHost extends EventEmitter {
  constructor(options) {
    super();
    const opts = options || {};
    this.spawnFn = opts.spawnFn || spawn;
    this.script = opts.script;
    this.env = childEnv(opts.env);
    this.logger = opts.logger || { info() {}, warn() {} };
    this.child = null;
    this.pid = null;
    this.stopped = null;
    this.exitCode = null;
    this.closed = false;
    // Событие stopped: mute игры, возможно, остался у Windows (null — помощник не сообщил).
    this.pending = null;
  }

  send(command) {
    try {
      if (this.child && this.child.stdin.writable) this.child.stdin.write(JSON.stringify(command) + "\n");
    } catch (error) {
      /* помощник уже вышел */
    }
  }

  // { exe, cwd, commandLine, mute, unmuteAll, resetCommandLine, resetExe?, followJob? } -> pid
  // запущенного (followJob: это посредник, HL1 Injector; игра жива, пока в задании есть процессы,
  // сброс mute запускает resetExe — саму игру).
  start(request) {
    if (!this.script) return Promise.reject(new DemoError("RENDER_HOST_MISSING"));
    return new Promise((resolve, reject) => {
      let settled = false;
      const fail = (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        reject(error);
      };
      const timer = setTimeout(() => {
        fail(new DemoError("RENDER_HOST_FAILED", "Помощник не ответил"));
        this.kill();
      }, READY_TIMEOUT_MS);
      try {
        this.child = this.spawnFn(powershellPath(this.env),
          ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", this.script],
          { windowsHide: true, shell: false, stdio: ["pipe", "pipe", "pipe"], env: this.env });
      } catch (error) {
        return fail(new DemoError("RENDER_HOST_FAILED", error.message));
      }
      const child = this.child;
      let pending = "";
      let stderr = "";
      child.stdout.setEncoding("utf8");
      child.stderr.setEncoding("utf8");
      child.stdin.on("error", () => {});
      child.stderr.on("data", (chunk) => {
        stderr = (stderr + chunk).slice(-2000);
      });
      child.stdout.on("data", (chunk) => {
        pending += chunk;
        let newline;
        while ((newline = pending.indexOf("\n")) >= 0) {
          const line = pending.slice(0, newline).trim();
          pending = pending.slice(newline + 1);
          if (!line) continue;
          let message;
          try {
            message = JSON.parse(line);
          } catch (error) {
            this.logger.warn(`[demo] помощник: ${line.slice(0, 200)}`);
            continue;
          }
          this.handle(message, request, resolve, fail, () => {
            settled = true;
            clearTimeout(timer);
          });
        }
      });
      child.on("error", (error) => fail(new DemoError("RENDER_HOST_FAILED", error.message)));
      child.on("close", (code) => {
        this.closed = true;
        if (!this.stopped) this.stopped = Promise.resolve();
        this.emit("closed", { code, stderr });
        fail(new DemoError("RENDER_HOST_FAILED", `Помощник вышел (${code}): ${stderr.trim().slice(-300)}`));
      });
    });
  }

  handle(message, request, resolve, fail, settle) {
    switch (message.event) {
      case "ready":
        this.send(Object.assign({ cmd: "start" }, request));
        break;
      case "started":
        this.pid = message.pid;
        settle();
        resolve(message.pid);
        break;
      case "error":
        this.logger.warn(`[demo] помощник: ошибка ${message.where || ""} ${message.code || message.message || ""}`);
        if (!this.pid) fail(new DemoError("RENDER_HOST_FAILED", `${message.where}: ${message.code || message.message}`));
        break;
      case "exited":
        this.exitCode = message.code;
        this.emit("exited", message);
        break;
      case "stopped":
        this.pending = message.pending === true;
        break;
      default:
        break;
    }
    this.emit("event", message);
  }

  // Закрыть игру (заглушённой) и стол; ждёт события "stopped". reset (по
  // умолчанию) — снять запомненный mute беззвучным перезапуском: конец рендера;
  // false — между пачками, следующая пачка стартует заглушённой.
  stop(reset) {
    if (this.stopped) return this.stopped;
    if (!this.child || this.closed) return (this.stopped = Promise.resolve());
    this.stopped = new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.kill();
        resolve();
      }, STOP_TIMEOUT_MS);
      const done = () => {
        clearTimeout(timer);
        resolve();
      };
      this.on("event", (message) => {
        if (message.event === "stopped") done();
      });
      this.once("closed", done);
      this.send({ cmd: "stop", reset: reset !== false });
    });
    return this.stopped;
  }

  get helperPid() {
    return this.child ? this.child.pid : null;
  }

  // Выход лаунчера: команда и закрытие stdin — помощник сам закроет
  // задание, снимет mute и закроет стол, даже если лаунчер уже вышел.
  detach() {
    this.send({ cmd: "stop" });
    try {
      if (this.child) this.child.stdin.end();
    } catch (error) {
      /* уже закрыт */
    }
  }

  kill() {
    try {
      if (this.child && !this.closed) this.child.kill();
    } catch (error) {
      /* уже вышел */
    }
  }
}

module.exports = { RenderHost, scriptPath, childEnv, STRIPPED_ENV };
