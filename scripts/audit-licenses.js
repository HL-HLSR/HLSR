#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const REPO_ROOT = path.resolve(__dirname, "..");
const NOTICES_PATH = path.join(REPO_ROOT, "build", "THIRD-PARTY-NOTICES.txt");

const STRONG_COPYLEFT = /(^|[^L])GPL|AGPL|SSPL|EUPL/i;
const WEAK_COPYLEFT = /LGPL|MPL|EPL|CDDL/i;

function loadLock() {
  const lock = JSON.parse(fs.readFileSync(path.join(REPO_ROOT, "package-lock.json"), "utf8"));
  if (!lock.packages) throw new Error("package-lock.json v1 не поддерживается — нужен lockfile v2/v3");
  return lock;
}

function productionPackages(lock) {
  const result = new Map();

  for (const [pkgPath, meta] of Object.entries(lock.packages)) {
    if (!pkgPath.startsWith("node_modules/")) continue;
    if (meta.dev || meta.devOptional) continue;

    const name = pkgPath.replace(/^node_modules\//, "");
    const packagePath = path.join(REPO_ROOT, pkgPath);
    let version = meta.version || null;
    if (!version) {
      try {
        version = JSON.parse(
          fs.readFileSync(path.join(packagePath, "package.json"), "utf8")
        ).version;
      } catch (ignored) {
      }
    }
    result.set(name, {
      name,
      version: version || "?",
      declaredLicense: meta.license || null,
      path: packagePath,
    });
  }

  return Array.from(result.values()).sort((a, b) => a.name.localeCompare(b.name));
}

function licenseOf(pkg) {
  if (pkg.declaredLicense) return pkg.declaredLicense;

  try {
    const meta = JSON.parse(fs.readFileSync(path.join(pkg.path, "package.json"), "utf8"));
    if (meta.license) return typeof meta.license === "string" ? meta.license : meta.license.type;
    if (Array.isArray(meta.licenses) && meta.licenses.length) return meta.licenses[0].type;
  } catch (ignored) {
  }

  for (const candidate of ["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"]) {
    const file = path.join(pkg.path, candidate);
    if (fs.existsSync(file)) {
      const firstLine = fs.readFileSync(file, "utf8").split(/\r?\n/).find((l) => l.trim());
      return `см. файл: ${firstLine ? firstLine.trim().slice(0, 60) : candidate}`;
    }
  }

  return null;
}

const INVENTORY_BEGIN = "=== BEGIN GENERATED DEPENDENCY INVENTORY (npm run audit:licenses -- --write-inventory) ===";
const INVENTORY_END = "=== END GENERATED DEPENDENCY INVENTORY ===";

function writeInventory(rows) {
  const lines = [
    INVENTORY_BEGIN,
    "",
    "Full production dependency tree (transitive included), generated " +
      new Date().toISOString().slice(0, 10) + ":",
    "",
  ];

  for (const row of rows)
    lines.push(`${(row.name + "@" + row.version).padEnd(48)} ${row.license}`);

  lines.push("", INVENTORY_END);
  const block = lines.join("\n");

  let body = fs.existsSync(NOTICES_PATH) ? fs.readFileSync(NOTICES_PATH, "utf8") : "";
  const begin = body.indexOf(INVENTORY_BEGIN);
  const end = body.indexOf(INVENTORY_END);

  if (begin !== -1 && end !== -1 && end > begin)
    body = body.slice(0, begin) + block + body.slice(end + INVENTORY_END.length);
  else body = body.replace(/\s*$/, "") + "\n\n" + block + "\n";

  fs.writeFileSync(NOTICES_PATH, body, "utf8");
  process.stdout.write(`Инвентарь из ${rows.length} пакетов записан в ${NOTICES_PATH}\n`);
}

function main() {
  const writeFlag = process.argv.includes("--write-inventory");

  const acknowledged = [];
  for (let i = 0; i < process.argv.length; i++)
    if (process.argv[i] === "--allow-unknown" && process.argv[i + 1])
      acknowledged.push(process.argv[++i]);
  const lock = loadLock();
  const packages = productionPackages(lock);

  const notices = fs.existsSync(NOTICES_PATH) ? fs.readFileSync(NOTICES_PATH, "utf8") : "";

  const rows = [];
  const problems = [];
  const missingInNotices = [];

  for (const pkg of packages) {
    const license = licenseOf(pkg) || "НЕИЗВЕСТНО";
    rows.push({ name: pkg.name, version: pkg.version, license });

    if (license === "НЕИЗВЕСТНО") problems.push(`нет данных о лицензии: ${pkg.name}@${pkg.version}`);
    else if (STRONG_COPYLEFT.test(license) && !/LGPL/i.test(license))
      problems.push(`СИЛЬНЫЙ COPYLEFT: ${pkg.name}@${pkg.version} (${license})`);
    else if (WEAK_COPYLEFT.test(license))
      problems.push(`слабый copyleft (нужно решение): ${pkg.name}@${pkg.version} (${license})`);

    if (!pkg.name.includes("/node_modules/") && !pkg.name.startsWith("@")) {
      const shortName = pkg.name.split("/").pop();
      if (notices && notices.toLowerCase().indexOf(shortName.toLowerCase()) === -1)
        missingInNotices.push(`${pkg.name}@${pkg.version} (${license})`);
    }
  }

  process.stdout.write(`Production-пакетов в дереве: ${rows.length}\n\n`);

  const byLicense = new Map();
  for (const row of rows) {
    const key = row.license;
    byLicense.set(key, (byLicense.get(key) || 0) + 1);
  }

  for (const [license, count] of Array.from(byLicense.entries()).sort((a, b) => b[1] - a[1]))
    process.stdout.write(`  ${String(count).padStart(4)}  ${license}\n`);

  if (missingInNotices.length) {
    process.stdout.write(`\nНе упомянуты в THIRD-PARTY-NOTICES.txt (${missingInNotices.length}):\n`);
    for (const name of missingInNotices) process.stdout.write(`  - ${name}\n`);
  }

  if (problems.length) {
    process.stdout.write("\nПроблемы лицензий:\n");
    for (const problem of problems) process.stdout.write(`  - ${problem}\n`);
  }

  if (writeFlag) writeInventory(rows);

  const blocking = problems.filter((p) => p.startsWith("СИЛЬНЫЙ"));

  const unknown = problems
    .filter((p) => p.startsWith("нет данных"))
    .map((p) => p.replace(/^нет данных о лицензии:\s*/, "").trim());

  const unacknowledged = unknown.filter((name) => acknowledged.indexOf(name) === -1);

  if (unknown.length) {
    process.stdout.write("\nЛицензия не подтверждена (право на распространение не доказано):\n");
    for (const name of unknown)
      process.stdout.write(
        `  - ${name}${acknowledged.indexOf(name) !== -1 ? "  [признано владельцем]" : ""}\n`
      );
    if (unacknowledged.length)
      process.stdout.write(
        "  Подтвердить осознанно: --allow-unknown <имя@версия> (и зафиксировать в отчёте)\n"
      );
  }

  process.stdout.write(
    `\nИтог: ${blocking.length ? "ЕСТЬ БЛОКИРУЮЩИЙ COPYLEFT" : "блокирующего copyleft нет"}, ` +
      `неизвестных лицензий: ${unknown.length}` +
      `${unacknowledged.length ? ` (не признано: ${unacknowledged.length})` : ""}, ` +
      `нет в notices: ${missingInNotices.length}\n`
  );

  if (blocking.length || unacknowledged.length) return 1;
  return missingInNotices.length && !writeFlag ? 1 : 0;
}

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`Аудит лицензий не выполнен: ${error.message}\n`);
    process.exitCode = 2;
  }
}
