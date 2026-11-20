"use strict";

const net = require("net");
const crypto = require("crypto");

const protocol = require("./protocol");

const DEFAULTS = {
  handshakeTimeoutMs: 5000,
  idleTimeoutMs: 30000,
  maxMessagesPerSecond: 20,
  maxLineBytes: protocol.MAX_LINE_BYTES,
};

function newToken() {
  return crypto.randomBytes(32).toString("hex");
}

function newPipeName() {
  return `\\\\.\\pipe\\hlsr-presence-${crypto.randomBytes(8).toString("hex")}`;
}

function tokenMatches(expected, received) {
  if (typeof expected !== "string" || typeof received !== "string") return false;
  if (expected.length !== received.length) return false;
  return crypto.timingSafeEqual(Buffer.from(expected, "utf8"), Buffer.from(received, "utf8"));
}

class PresenceBridgeServer {
  constructor(options) {
    const opts = options || {};
    this.log = opts.log || { info() {}, warn() {}, error() {} };
    this.now = opts.now || (() => Date.now());
    this.setTimer = opts.setTimeout || setTimeout;
    this.clearTimer = opts.clearTimeout || clearTimeout;

    this.limits = Object.assign({}, DEFAULTS, opts.limits || {});

    this.platform = opts.platform || process.platform;
    this.token = opts.token || newToken();
    this.pipeName = opts.pipeName || newPipeName();
    // Порт прошлого запуска: мост под Proton у уже идущей игры знает только его.
    this.preferredPort = Number.isInteger(opts.port) && opts.port > 0 ? opts.port : 0;

    this.server = null;
    this.connection = null;

    this.stateHandler = opts.onState || (() => {});
    this.gameGoneHandler = opts.onGameGone || (() => {});
    this.chatHandler = opts.onChat || (() => {});
    this.gameReadyHandler = opts.onGameReady || (() => {});

    this.discordConnected = false;

    this.stats = { accepted: 0, rejected: 0, messages: 0, dropped: 0 };
  }

  start() {
    if (this.server) return this.pipeName;

    this.server = net.createServer((socket) => this.onConnection(socket));
    this.server.on("error", (error) => {
      const code = error && error.code ? error.code : "ERR";
      // Старый порт заняли — слушаем новый: игра прошлого запуска статус
      // уже не передаст, новые запуски получат свежий адрес.
      if (code === "EADDRINUSE" && this.platform !== "win32" && this.preferredPort && !this.server.listening) {
        this.log.warn(`[game-presence] порт ${this.preferredPort} занят, беру новый`);
        this.preferredPort = 0;
        this.server.listen({ host: "127.0.0.1", port: 0, exclusive: true });
        return;
      }
      this.log.warn(`[game-presence] канал недоступен: ${code}`);
    });
    // На Windows игра подключается к именованному каналу напрямую. На Linux
    // игра идёт под Proton, и канал \\.\pipe\... живёт внутри wineserver:
    // его создаёт hlsr-presence-relay.exe в префиксе игры и пересылает байты
    // сюда, на 127.0.0.1. Порт выбирает система.
    if (this.platform === "win32") this.server.listen(this.pipeName);
    else this.server.listen({ host: "127.0.0.1", port: this.preferredPort, exclusive: true });
    return this.pipeName;
  }

  stop() {
    this.closeConnection("SHUTDOWN");
    if (this.server) {
      try {
        this.server.close();
      } catch (e) {
        /* уже закрыт */
      }
      this.server = null;
    }
  }

  endpoint() {
    return { pipeName: this.pipeName, token: this.token, port: this.port() };
  }

  port() {
    if (!this.server || this.platform === "win32") return null;
    const address = this.server.address();
    return address && typeof address === "object" ? address.port : null;
  }

  environment() {
    if (!this.server) return {};
    if (this.platform === "win32") return { HLSR_PRESENCE_PIPE: this.pipeName, HLSR_PRESENCE_TOKEN: this.token };
    const port = this.port();
    if (!port) return {};
    return { HLSR_PRESENCE_PIPE: this.pipeName, HLSR_PRESENCE_TOKEN: this.token, HLSR_PRESENCE_PORT: String(port) };
  }

  onConnection(socket) {
    // TCP на Linux доступен любому локальному процессу: пока игра на связи,
    // чужое соединение её не вытесняет. Именованный канал на Windows ведёт
    // себя как раньше.
    if (this.platform !== "win32" && this.connection && this.connection.authenticated) {
      this.stats.rejected++;
      socket.destroy();
      return;
    }
    if (this.connection) this.closeConnection("REPLACED");

    const conn = {
      socket,
      authenticated: false,
      session: null,
      buffer: "",
      windowStartMs: this.now(),
      windowCount: 0,
      timer: null,
    };
    this.connection = conn;

    socket.setNoDelay(true);
    socket.setEncoding("utf8");

    this.armTimeout(conn, this.limits.handshakeTimeoutMs, "HANDSHAKE_TIMEOUT");

    socket.on("data", (chunk) => this.onData(conn, chunk));
    socket.on("error", () => this.dropConnection(conn, "SOCKET_ERROR"));
    socket.on("close", () => this.dropConnection(conn, "CLOSED"));
  }

  armTimeout(conn, ms, reason) {
    if (conn.timer) this.clearTimer(conn.timer);
    conn.timer = this.setTimer(() => {
      conn.timer = null;
      this.reject(conn, reason);
    }, ms);
  }

  onData(conn, chunk) {
    if (this.connection !== conn) return;

    conn.buffer += chunk;

    if (Buffer.byteLength(conn.buffer, "utf8") > this.limits.maxLineBytes * 2)
      return this.reject(conn, "LINE_TOO_LONG");

    let index;
    while ((index = conn.buffer.indexOf("\n")) !== -1) {
      const line = conn.buffer.slice(0, index).replace(/\r$/, "");
      conn.buffer = conn.buffer.slice(index + 1);
      if (line.length === 0) continue;
      if (!this.onLine(conn, line)) return;
    }
  }

  onLine(conn, line) {
    const at = this.now();

    if (at - conn.windowStartMs >= 1000) {
      conn.windowStartMs = at;
      conn.windowCount = 0;
    }
    conn.windowCount++;
    if (conn.windowCount > this.limits.maxMessagesPerSecond) {
      this.reject(conn, "RATE_LIMIT");
      return false;
    }

    const parsed = protocol.parseLine(line);
    if (!parsed.ok) {
      this.reject(conn, parsed.reason);
      return false;
    }

    const message = parsed.message;

    if (!conn.authenticated) {
      if (message.type !== "hello") {
        this.reject(conn, "NO_HANDSHAKE");
        return false;
      }
      if (!tokenMatches(this.token, message.token)) {
        this.reject(conn, "BAD_TOKEN");
        return false;
      }
      conn.authenticated = true;
      conn.session = message.session;
      this.stats.accepted++;
      this.armTimeout(conn, this.limits.idleTimeoutMs, "IDLE_TIMEOUT");
      this.sendDiscordState(this.discordConnected);

      try {
        this.gameReadyHandler();
      } catch (error) {
        this.log.warn(`[game-presence] обработчик рукопожатия упал: ${error.message}`);
      }
      return true;
    }

    if (message.type === "hello") {
      this.reject(conn, "DUPLICATE_HANDSHAKE");
      return false;
    }

    if (message.session !== conn.session) {
      this.stats.dropped++;
      return true;
    }

    this.stats.messages++;
    this.armTimeout(conn, this.limits.idleTimeoutMs, "IDLE_TIMEOUT");

    if (message.type !== "state") {
      try {
        this.chatHandler(message);
      } catch (error) {
        this.log.warn(`[game-presence] обработчик чата упал: ${error.message}`);
      }
      return true;
    }

    try {
      this.stateHandler(message);
    } catch (error) {
      this.log.warn(`[game-presence] обработчик состояния упал: ${error.message}`);
    }
    return true;
  }

  reject(conn, reason) {
    if (conn.authenticated) this.log.info(`[game-presence] соединение закрыто: ${reason}`);
    else this.stats.rejected++;
    this.dropConnection(conn, reason);
  }

  dropConnection(conn, reason) {
    if (conn.timer) {
      this.clearTimer(conn.timer);
      conn.timer = null;
    }
    try {
      conn.socket.destroy();
    } catch (e) {
      /* уже закрыт */
    }
    if (this.connection !== conn) return;
    this.connection = null;

    try {
      this.gameGoneHandler(reason);
    } catch (error) {
      this.log.warn(`[game-presence] обработчик отключения упал: ${error.message}`);
    }
  }

  closeConnection(reason) {
    if (this.connection) this.dropConnection(this.connection, reason);
  }

  setDiscordConnected(connected) {
    const next = connected === true;
    if (next === this.discordConnected) return false;
    this.discordConnected = next;
    this.sendDiscordState(next);
    return true;
  }

  sendDiscordState(connected) {
    return this.push(protocol.encodeDiscordAck(connected));
  }

  push(line) {
    const conn = this.connection;
    if (!conn || !conn.authenticated) return false;
    try {
      conn.socket.write(line);
    } catch (e) {
      return false;
    }
    return true;
  }

  sendChatLine(platform, nick, text, badge1, badge2) {
    return this.push(protocol.encodeChatLine(platform, nick, text, badge1, badge2));
  }

  sendEmoteReady(name, wide, tall, rgbaBase64) {
    return this.push(protocol.encodeEmoteReady(name, wide, tall, rgbaBase64));
  }

  sendBadgeReady(name, wide, tall, rgbaBase64) {
    return this.push(protocol.encodeBadgeReady(name, wide, tall, rgbaBase64));
  }

  sendEmoteBegin(name, wide, tall, frames, frameMs) {
    return this.push(protocol.encodeEmoteBegin(name, wide, tall, frames, frameMs));
  }

  sendEmoteFrame(name, index, rgbaBase64) {
    return this.push(protocol.encodeEmoteFrame(name, index, rgbaBase64));
  }

  sendEmoteMiss(name) {
    return this.push(protocol.encodeEmoteMiss(name));
  }

  diagnostics() {
    return {
      listening: Boolean(this.server),
      connected: Boolean(this.connection && this.connection.authenticated),
      accepted: this.stats.accepted,
      rejected: this.stats.rejected,
      messages: this.stats.messages,
      dropped: this.stats.dropped,
    };
  }
}

module.exports = { PresenceBridgeServer, newToken, newPipeName, tokenMatches, DEFAULTS };
