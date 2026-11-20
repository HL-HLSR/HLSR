"use strict";

// Канал связи с игрой (имя канала, токен, порт на Linux) живёт в окружении
// уже запущенной игры. Если лаунчер закрыли посреди игры, после перезапуска
// он должен поднять тот же канал, иначе игра и мост под Proton стучатся в
// старый адрес и статус в Discord отвязывается. Файл хранится, пока игра
// идёт; при выходе без запущенных игр он удаляется, и токен меняется.

const fs = require("fs");
const { writeFileAtomic } = require("../game-delivery/atomic");

const PIPE_RE = /^\\\\\.\\pipe\\hlsr-presence-[0-9a-f]{16}$/;
const TOKEN_RE = /^[0-9a-f]{64}$/;

function normalize(value) {
  if (!value || typeof value !== "object") return null;
  if (!PIPE_RE.test(String(value.pipeName || "")) || !TOKEN_RE.test(String(value.token || ""))) return null;
  const port = Number(value.port);
  return {
    pipeName: value.pipeName,
    token: value.token,
    port: Number.isInteger(port) && port > 0 && port <= 65535 ? port : null,
  };
}

function load(file) {
  try {
    return normalize(JSON.parse(fs.readFileSync(file, "utf8")));
  } catch (error) {
    return null;
  }
}

function save(file, endpoint) {
  const value = normalize(endpoint);
  if (!value) return false;
  writeFileAtomic(file, JSON.stringify(value));
  try {
    fs.chmodSync(file, 0o600);
  } catch (error) {
    /* на Windows права задаёт профиль пользователя */
  }
  return true;
}

function clear(file) {
  try {
    fs.rmSync(file, { force: true });
  } catch (error) {
    /* нечего удалять */
  }
}

module.exports = { load, save, clear, normalize };
