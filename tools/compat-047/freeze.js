#!/usr/bin/env node
"use strict";

// Замороженные копии модулей обновлялки (src/**) для приёмки выпуска, по набору на версию:
// 0.4.7 — src/ (она же проверяет базу 0.4.8: в этих файлах 0.4.8 = 0.4.7), 0.4.9 — src-049/.
// Каталог набора повторяет раскладку эталона, поэтому относительные require() работают как есть.
// Копии — честный эталон: обновлялка у пользователей не меняется. Список и наборы — frozen.json.
//
//   node tools/compat-047/freeze.js --check            # все наборы, побайтно с эталонами
//   node tools/compat-047/freeze.js --check 0.4.9      # один набор
//   node tools/compat-047/freeze.js --freeze 0.4.9     # скопировать набор из эталона заново
//
// Эталон набора — распакованная копия лаунчера этой версии: каталог из переменной набора
// (HLSR_ELECTRON для 0.4.7, HLSR_ELECTRON_049 для 0.4.9), иначе ~/HLSR-electron, ~/HLSR-electron-049.
// Копия другой версии (по package.json) — не эталон этого набора: сверка пропускается с причиной.
// --check падает на любом расхождении, лишнем или недостающем файле; если сверить не с чем ни
// для одного набора — код 3 (тест это пропускает, а не проваливает).

const fs = require("fs");
const os = require("os");
const path = require("path");

const ROOT = __dirname;
const LIST = JSON.parse(fs.readFileSync(path.join(ROOT, "frozen.json"), "utf8"));
const SETS = Object.keys(LIST.sets);

function setOf(name) {
  const set = LIST.sets[name];
  if (!set) throw new Error(`нет набора ${name}: есть ${SETS.join(", ")}`);
  return set;
}

/** Каталог копий набора. */
const setDir = (name) => path.join(ROOT, setOf(name).dir);

/** Каталог эталона набора (копия лаунчера этой версии). */
function referenceRoot(name) {
  const set = setOf(name);
  return path.resolve(process.env[set.env] || path.join(os.homedir(), set.home));
}

/** Почему сверить набор не с чем, или null, если эталон на месте и той же версии. */
function referenceMissing(name) {
  const root = referenceRoot(name);
  if (!fs.existsSync(path.join(root, "src", LIST.files[0]))) return `нет эталонной копии ${root}`;
  let version = null;
  try { version = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).version; } catch {}
  return version === name ? null : `${root} — версия ${version}, а набор ${name}`;
}

/** Набор для версии лаунчера у пользователя (0.4.8 → 0.4.7). */
function setForBase(base) {
  const name = LIST.bases[base];
  if (!name) throw new Error(`база ${base} не поддерживается: есть ${Object.keys(LIST.bases).join(", ")}`);
  return name;
}

function listFiles(dir, base = dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) listFiles(full, base, out);
    else out.push(path.relative(base, full).split(path.sep).join("/"));
  }
  return out;
}

/** Расхождения копий набора с эталоном и со списком: массив строк, пусто — всё сошлось. */
function check(name, { compareWithReference = true } = {}) {
  const dir = setDir(name);
  const reference = referenceRoot(name);
  const problems = [];
  const present = fs.existsSync(dir) ? new Set(listFiles(dir)) : new Set();
  for (const file of LIST.files) {
    if (!present.has(file)) { problems.push(`${name}: нет копии: ${file}`); continue; }
    if (!compareWithReference) continue;
    const source = path.join(reference, "src", file);
    if (!fs.existsSync(source)) { problems.push(`${name}: нет в эталоне: ${file}`); continue; }
    if (Buffer.compare(fs.readFileSync(source), fs.readFileSync(path.join(dir, file))) !== 0)
      problems.push(`${name}: не совпадает с эталоном: ${file}`);
  }
  for (const file of present) if (!LIST.files.includes(file)) problems.push(`${name}: лишний файл: ${file}`);
  return problems;
}

function freeze(name) {
  const missing = referenceMissing(name);
  if (missing) throw new Error(missing);
  for (const file of LIST.files) {
    const target = path.join(setDir(name), file);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(path.join(referenceRoot(name), "src", file), target);
  }
}

module.exports = {
  ROOT, FILES: LIST.files, SETS, BASES: LIST.bases, setOf, setDir, referenceRoot, referenceMissing, setForBase, check, freeze,
};

if (require.main === module) {
  const [mode, only] = process.argv.slice(2);
  try {
    if (mode === "--freeze" && only) {
      freeze(only);
      console.log(`${only}: скопировано ${LIST.files.length} файлов из ${referenceRoot(only)}`);
    } else if (mode === "--check") {
      let checked = 0;
      let problems = 0;
      for (const name of only ? [only] : SETS) {
        const missing = referenceMissing(name);
        if (missing) { console.error(`${name}: ${missing}: сверка пропущена`); continue; }
        checked++;
        const found = check(name);
        for (const problem of found) console.error(problem);
        problems += found.length;
        if (!found.length) console.log(`${name}: ${LIST.files.length} файлов совпадают с эталоном ${setOf(name).reference}`);
      }
      if (problems) console.log(`расхождений: ${problems}`);
      process.exitCode = problems ? 1 : checked ? 0 : 3;
    } else {
      console.error(`node tools/compat-047/freeze.js --check [набор] | --freeze <набор>   (наборы: ${SETS.join(", ")})`);
      process.exitCode = 2;
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 2;
  }
}
