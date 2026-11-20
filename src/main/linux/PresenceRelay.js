"use strict";

// Мост статуса игры для Discord под Proton. client.dll Ghosting шлёт карту,
// FPS и таймер забега в именованный канал из HLSR_PRESENCE_PIPE, а под Wine
// такой канал живёт внутри wineserver и с хоста не виден. Поэтому в префикс
// игры до неё запускается hlsr-presence-relay.exe (scripts/presence-relay):
// он создаёт канал и пересылает байты на 127.0.0.1:<порт>, где их принимает
// PresenceBridgeServer. Токен помощнику не нужен: рукопожатие проверяет
// лаунчер.

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { waitForStatus } = require("../game-delivery/livesplit-overlay");

const RESOURCE_DIRECTORY = "presence-relay";
const EXECUTABLE = "hlsr-presence-relay.exe";
const PIPE_RE = /^\\\\\.\\pipe\\hlsr-presence-[0-9a-f]{8,64}$/;

// Окружение моста (PresenceBridgeServer.environment) → куда слушать и что
// отдать игре. null — мост на хосте не поднят.
function targetOf(environment) {
  const env = environment || {};
  const port = Number(env.HLSR_PRESENCE_PORT);
  if (!PIPE_RE.test(String(env.HLSR_PRESENCE_PIPE || "")) || !Number.isInteger(port) || port <= 0 || port > 65535 ||
      typeof env.HLSR_PRESENCE_TOKEN !== "string" || !env.HLSR_PRESENCE_TOKEN)
    return null;
  return {
    pipe: env.HLSR_PRESENCE_PIPE,
    port,
    gameEnv: { HLSR_PRESENCE_PIPE: env.HLSR_PRESENCE_PIPE, HLSR_PRESENCE_TOKEN: env.HLSR_PRESENCE_TOKEN },
  };
}

function relayArguments(target, processName, statusPath) {
  return ["--pipe", target.pipe, "--port", String(target.port), "--name", processName, "--status", statusPath];
}

function sha256Of(fsModule, file) {
  return crypto.createHash("sha256").update(fsModule.readFileSync(file)).digest("hex");
}

class PresenceRelay {
  constructor(options = {}) {
    this.baseDir = options.baseDir;
    this.resourcesPath = options.resourcesPath || process.resourcesPath;
    this.fs = options.fs || fs;
  }

  // Копия помощника в папке данных лаунчера: ресурсы AppImage смонтированы
  // во временную папку, которая живёт, только пока работает лаунчер.
  prepare() {
    const source = path.join(this.resourcesPath, RESOURCE_DIRECTORY, EXECUTABLE);
    if (!this.baseDir) throw new Error("папка данных лаунчера не задана");
    if (!this.fs.existsSync(source)) throw new Error(`${EXECUTABLE} не найден в ресурсах лаунчера`);
    const dir = path.join(this.baseDir, RESOURCE_DIRECTORY);
    this.fs.mkdirSync(dir, { recursive: true });
    const executable = path.join(dir, EXECUTABLE);
    let current = null;
    try { current = sha256Of(this.fs, executable); } catch (error) { /* копии ещё нет */ }
    if (current !== sha256Of(this.fs, source)) {
      const temporary = `${executable}.${process.pid}.tmp`;
      this.fs.copyFileSync(source, temporary);
      this.fs.renameSync(temporary, executable);
    }
    const status = path.join(dir, "relay.status");
    this.fs.rmSync(status, { force: true });
    return { dir, executable, status };
  }

  waitForStatus(file, timeoutMs, options = {}) {
    return waitForStatus(this.fs, file, timeoutMs, options);
  }
}

module.exports = PresenceRelay;
module.exports.targetOf = targetOf;
module.exports.relayArguments = relayArguments;
module.exports.RESOURCE_DIRECTORY = RESOURCE_DIRECTORY;
module.exports.EXECUTABLE = EXECUTABLE;
