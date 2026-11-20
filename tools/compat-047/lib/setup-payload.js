"use strict";

// Чтение нагрузки установщика Windows (решение Р1; формат — crates/hlsr-update/src/windows/payload.rs):
//
//   [HLSR.exe: N байт][нагрузка: L байт][хвост: 60 байт]
//   нагрузка = u64 LE длина списка, install-files.json, файлы списка подряд (кроме HLSR.exe)
//   хвост    = "HLSR-SETUP-1", u64 LE смещение (N), u64 LE длина (L), sha256 нагрузки
//
// Свой разбор на Node нужен приёмке: она проверяет выпуск не кодом, который его упаковал, а
// независимо, с проверкой списка файлов замороженным file-list.js набора базы.

const crypto = require("crypto");
const fs = require("fs");

const SIGNATURE = "HLSR-SETUP-1";
const TRAILER_LEN = 60;
const CHUNK = 1024 * 1024;

function hashRange(fd, start, length) {
  const hash = crypto.createHash("sha256");
  const buffer = Buffer.allocUnsafe(Math.min(CHUNK, Math.max(1, length)));
  let position = start;
  let rest = length;
  while (rest > 0) {
    const want = Math.min(buffer.length, rest);
    const read = fs.readSync(fd, buffer, 0, want, position);
    if (read <= 0) throw new Error("файл короче, чем описано в хвосте");
    hash.update(buffer.subarray(0, read));
    position += read;
    rest -= read;
  }
  return hash.digest("hex");
}

function safe(value, what) {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error(`${what} не помещается в число`);
  return Number(value);
}

/**
 * Разбор и полная проверка нагрузки: хвост, sha256 нагрузки, список файлов, размеры и sha256
 * каждого файла, sha256 HLSR.exe (первые N байт). Возвращает { size, exeLength, exeSha256, list,
 * listBytes, files } или бросает Error с причиной.
 */
function readSetup(file, { problemsOf, set }) {
  const fd = fs.openSync(file, "r");
  try {
    const size = fs.fstatSync(fd).size;
    if (size < TRAILER_LEN + 8) throw new Error("файл слишком мал для установщика");
    const trailer = Buffer.alloc(TRAILER_LEN);
    fs.readSync(fd, trailer, 0, TRAILER_LEN, size - TRAILER_LEN);
    if (trailer.subarray(0, 12).toString("latin1") !== SIGNATURE) throw new Error("нет хвоста HLSR-SETUP-1: это не установщик с нагрузкой");
    const exeLength = safe(trailer.readBigUInt64LE(12), "смещение нагрузки");
    const length = safe(trailer.readBigUInt64LE(20), "длина нагрузки");
    const declared = trailer.subarray(28).toString("hex");
    if (exeLength === 0 || length < 8 || exeLength + length + TRAILER_LEN !== size)
      throw new Error("смещение и длина нагрузки не сходятся с размером файла");
    if (hashRange(fd, exeLength, length) !== declared) throw new Error("sha256 нагрузки не совпал с хвостом");

    const head = Buffer.alloc(8);
    fs.readSync(fd, head, 0, 8, exeLength);
    const listLength = safe(head.readBigUInt64LE(0), "длина списка");
    if (listLength > length - 8) throw new Error("список файлов длиннее нагрузки");
    const listBytes = Buffer.alloc(listLength);
    fs.readSync(fd, listBytes, 0, listLength, exeLength + 8);
    const list = JSON.parse(listBytes.toString("utf8"));
    const problems = problemsOf(list);
    if (problems.length) throw new Error(`список файлов некорректен (file-list.js ${set}): ${problems.slice(0, 3).join("; ")}`);
    if (listBytes[listBytes.length - 1] !== 0x0a || JSON.stringify(list) + "\n" !== listBytes.toString("utf8"))
      throw new Error("install-files.json — не JSON.stringify(list) + перевод строки");

    let cursor = exeLength + 8 + listLength;
    const end = exeLength + length;
    const files = [];
    for (const item of list.files) {
      if (item.path === "HLSR.exe") {
        const sha = hashRange(fd, 0, exeLength);
        if (item.size !== exeLength) throw new Error(`HLSR.exe: в списке ${item.size} байт, в установщике ${exeLength}`);
        if (sha !== item.sha256) throw new Error("HLSR.exe: sha256 первых N байт не совпал со списком");
        files.push({ path: item.path, size: item.size, sha256: sha, offset: 0 });
        continue;
      }
      if (cursor + item.size > end) throw new Error(`${item.path}: выходит за конец нагрузки`);
      const sha = hashRange(fd, cursor, item.size);
      if (sha !== item.sha256) throw new Error(`${item.path}: sha256 не совпал со списком`);
      files.push({ path: item.path, size: item.size, sha256: sha, offset: cursor });
      cursor += item.size;
    }
    if (cursor !== end) throw new Error("после файлов списка в нагрузке остались лишние байты");
    if (!files.some((item) => item.path === "HLSR.exe")) throw new Error("в списке нет HLSR.exe");
    return { size, exeLength, exeSha256: files.find((item) => item.path === "HLSR.exe").sha256, list, listBytes, files };
  } finally {
    fs.closeSync(fd);
  }
}

module.exports = { readSetup, SIGNATURE, TRAILER_LEN };
