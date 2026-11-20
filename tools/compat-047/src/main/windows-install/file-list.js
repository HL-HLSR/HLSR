"use strict";

// Список файлов Windows-сборки: путь, размер и SHA-256 каждого файла.
// Пишется при сборке в resources/install-files.json и едет внутри
// установщика; установка копирует ровно эти файлы и сверяет каждый, так
// что битая распаковка или файл, унесённый антивирусом, не заменят
// рабочую установку. Там же appId и имя продукта сборки: по ним
// установщик выбирает ключи реестра и каталог (layout.identity).
const crypto = require("crypto");
const path = require("path");
const fs = require("../launcher-update/fs-compat");
const layout = require("./layout");

const FILE = "install-files.json";
const RELATIVE = `resources/${FILE}`;
const SCHEMA = 1;
const SHA256_RE = /^[a-f0-9]{64}$/;

function walk(root, relative, out) {
  for (const entry of fs.readdirSync(path.join(root, relative), { withFileTypes: true })) {
    const child = relative ? `${relative}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`Ссылка в сборке: ${child}`);
    if (entry.isDirectory()) walk(root, child, out);
    else if (entry.isFile()) out.push(child);
  }
  return out;
}

function sha256File(file) {
  const hash = crypto.createHash("sha256");
  const fd = fs.openSync(file, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    let read;
    while ((read = fs.readSync(fd, buffer, 0, buffer.length, null)) > 0) hash.update(buffer.subarray(0, read));
  } finally {
    fs.closeSync(fd);
  }
  return hash.digest("hex");
}

function build(root, version, app) {
  const files = walk(root, "", [])
    .filter((file) => file !== RELATIVE)
    .sort()
    .map((file) => {
      const full = path.join(root, file);
      return { path: file, size: fs.statSync(full).size, sha256: sha256File(full) };
    });
  return { schema: SCHEMA, version, app: { appId: app.appId, productName: app.productName }, files };
}

function problemsOf(data) {
  if (!data || data.schema !== SCHEMA || !Array.isArray(data.files)) return ["неизвестная схема списка файлов"];
  if (!/^\d+\.\d+\.\d+$/.test(data.version || "")) return [`некорректная версия: ${data.version}`];
  const problems = [];
  try {
    if (!data.app || typeof data.app.appId !== "string" || !data.app.appId) throw new Error("нет appId сборки");
    layout.identity(data.app);
  } catch (error) {
    problems.push(error.message);
  }
  const seen = new Set();
  for (const item of data.files) {
    const file = item && item.path;
    if (typeof file !== "string" || !file || file.startsWith("/") || /(^|\/)\.\.(\/|$)/.test(file) || /[\\:]/.test(file))
      problems.push(`недопустимый путь: ${file}`);
    else if (seen.has(file.toLowerCase())) problems.push(`повтор: ${file}`);
    else seen.add(file.toLowerCase());
    if (!Number.isSafeInteger(item && item.size) || item.size < 0) problems.push(`некорректный размер: ${file}`);
    if (!SHA256_RE.test((item && item.sha256) || "")) problems.push(`некорректный SHA-256: ${file}`);
  }
  if (!data.files.some((item) => item.path === layout.EXE)) problems.push(`в списке нет ${layout.EXE}`);
  return problems;
}

function read(root) {
  let data;
  try {
    data = JSON.parse(fs.readFileSync(path.join(root, RELATIVE), "utf8"));
  } catch (error) {
    throw new Error(`Список файлов установщика не читается: ${error.message}`, { cause: error });
  }
  const problems = problemsOf(data);
  if (problems.length) throw new Error(`Список файлов установщика повреждён: ${problems.slice(0, 3).join("; ")}`);
  return data;
}

module.exports = { FILE, RELATIVE, build, read, problemsOf, sha256File };
