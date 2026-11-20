"use strict";

// Автономный установщик Linux, как установщик Windows: архив лаунчера вшит в
// AppImage (resources/payload) вместе с описанием — версия, размер, SHA-256 и
// SHA-512. Установщик ставит его без сети, а обновлятор скачивает AppImage по
// подписанному каналу, проверяет его хеши и достаёт тот же архив (extractFrom).
const fs = require("fs");
const path = require("path");
const { execFile } = require("child_process");

const DIRECTORY = "payload";
const DESCRIPTOR = "payload.json";
const SCHEMA = 1;
const MAX_UNPACKED_BYTES = 2 * 1024 * 1024 * 1024;

const VERSION_RE = /^\d+\.\d+\.\d+$/;
const SHA256_RE = /^[a-f0-9]{64}$/;
const SHA512_B64_RE = /^[A-Za-z0-9+/]{86}==$/;

function archiveName(version) {
  return `HLSR-${version}-linux-x86_64.tar.gz`;
}

function payloadDir(resourcesPath = process.resourcesPath) {
  return path.join(resourcesPath, DIRECTORY);
}

function describe({ version, size, sha256, sha512, unpackedSize }) {
  return {
    schema: SCHEMA,
    version,
    installer: { format: "tar.gz", filename: archiveName(version), size, sha256, sha512, unpackedSize },
  };
}

function problemsOf(data) {
  const installer = data && data.installer;
  if (!data || typeof data !== "object" || data.schema !== SCHEMA) return ["неизвестная схема описания"];
  if (!VERSION_RE.test(data.version || "")) return [`некорректная версия: ${data.version}`];
  if (!installer || typeof installer !== "object") return ["нет описания архива"];

  const problems = [];
  if (installer.format !== "tar.gz") problems.push("ожидался архив tar.gz");
  if (installer.filename !== archiveName(data.version)) problems.push(`неожиданное имя архива: ${installer.filename}`);
  if (!Number.isSafeInteger(installer.size) || installer.size <= 0) problems.push("некорректный размер архива");
  if (!SHA256_RE.test(installer.sha256 || "")) problems.push("некорректный SHA-256");
  if (!SHA512_B64_RE.test(installer.sha512 || "")) problems.push("некорректный SHA-512");
  if (!Number.isSafeInteger(installer.unpackedSize) || installer.unpackedSize <= 0 || installer.unpackedSize > MAX_UNPACKED_BYTES)
    problems.push("некорректный распакованный размер");
  return problems;
}

function read(directory = payloadDir()) {
  let data;
  try {
    data = JSON.parse(fs.readFileSync(path.join(directory, DESCRIPTOR), "utf8"));
  } catch (error) {
    throw new Error(`В установщике нет встроенного HLSR: ${error.message}`, { cause: error });
  }

  const problems = problemsOf(data);
  if (problems.length) throw new Error(`Описание встроенного HLSR повреждено: ${problems.join("; ")}`);

  return {
    manifest: { version: data.version, installer: data.installer },
    archive: path.join(directory, data.installer.filename),
  };
}

// Среда AppImage, из которого запущен лаунчер, не должна попасть в рантайм
// скачанного установщика.
function extractEnvironment(source = process.env) {
  const env = { ...source };
  for (const key of ["APPIMAGE", "APPDIR", "ARGV0", "OWD", "APPIMAGE_EXTRACT_AND_RUN"]) delete env[key];
  return env;
}

// Рантайм AppImage сам распаковывает squashfs без FUSE: --appimage-extract
// с префиксом пути кладёт только resources/payload в <directory>/squashfs-root.
// Вызывать только для AppImage, хеши которого уже сверены с подписанным каналом.
function extractFrom(appImage, directory, options = {}) {
  fs.chmodSync(appImage, 0o700);
  return new Promise((resolve, reject) => {
    execFile(appImage, ["--appimage-extract", `resources/${DIRECTORY}`], {
      cwd: directory, env: extractEnvironment(), signal: options.signal,
      timeout: options.timeout || 10 * 60 * 1000, maxBuffer: 64 * 1024 * 1024,
    }, (error) => {
      if (error) return reject(new Error(`Не удалось распаковать установщик: ${error.message}`));
      try { resolve(read(path.join(directory, "squashfs-root", "resources", DIRECTORY))); }
      catch (readError) { reject(readError); }
    });
  });
}

module.exports = { DIRECTORY, DESCRIPTOR, SCHEMA, archiveName, payloadDir, describe, problemsOf, read, extractFrom };
