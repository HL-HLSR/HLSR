#!/usr/bin/env node
"use strict";

// Копия scripts/publish-launcher-chunks.js 0.4.7 (подпроект 6, задача 8, решение Р8): модули чанков,
// канала и адресов - замороженные копии tools/compat-047 (lib/frozen.js), больше ничего не менялось.
// ПУБЛИКУЕТ НА СЕРВЕР: запускает только владелец, по своему "да" (ПП7).
//
// Выкладка чанков лаунчера на hlsr.pro. Запускать ДО публикации канала:
// канал с блоком delta отправляет клиентов за манифестом и кусками, и они
// должны уже лежать на сервере.
//
//   node tools/release/node/publish-launcher-chunks.js --plan release/launcher-linux/0.5.0/publish-plan.json \
//     --target ubuntu@host:/srv/hlsr/www/downloads [--sudo] [--publish]
//
// Без --publish только показывает, что уедет. Куски называются своим
// SHA-256 и не меняются, поэтому rsync --ignore-existing досылает только
// новые. После выкладки манифест и выборка кусков скачиваются по HTTPS и
// сверяются.
const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");
const { chunks, launcherChannel: channel, endpoints } = require("./lib/frozen");

const SAMPLE = 40;

function parseArgs(argv) {
  const args = { publish: false, sudo: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--plan") args.plan = argv[++i];
    else if (argv[i] === "--target") args.target = argv[++i];
    else if (argv[i] === "--publish") args.publish = true;
    else if (argv[i] === "--sudo") args.sudo = true;
    else throw new Error(`Неизвестный аргумент: ${argv[i]}`);
  }
  if (!args.plan || !args.target) throw new Error("Нужны --plan <publish-plan.json> и --target <user@host:/каталог downloads>");
  if (!/^[^:\s]+:\/\S+$/.test(args.target)) throw new Error(`--target должен быть вида user@host:/путь: ${args.target}`);
  return args;
}

function rsync(args, list, from, to, sudo) {
  const command = ["-a", "--chmod=D755,F644", "--ignore-existing", "--mkpath", "--itemize-changes"];
  if (sudo) command.push("--rsync-path=sudo rsync");
  if (list) command.push(`--files-from=${list}`);
  command.push(from, to);
  return execFileSync("rsync", command.concat(args), { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const planPath = path.resolve(args.plan);
  const plan = JSON.parse(fs.readFileSync(planPath, "utf8"));
  const artifact = (plan.artifacts || []).find((item) => item.kind === "launcher-chunk-manifest");
  if (!artifact || !plan.chunkStore) throw new Error("В плане нет чанков: релиз подготовлен с --no-chunks?");

  const manifestPath = path.resolve(path.dirname(planPath), artifact.localPath);
  const body = fs.readFileSync(manifestPath);
  const manifest = JSON.parse(body.toString("utf8"));
  const problems = chunks.problemsOf(manifest, { version: plan.version });
  if (problems.length) throw new Error(`Манифест чанков: ${problems.join("; ")}`);

  const store = plan.chunkStore.localDir;
  const hashes = new Map();
  for (const file of manifest.files) for (const [hash, size, csize] of file.chunks) hashes.set(hash, { size, csize });
  for (const hash of hashes.keys())
    if (!fs.existsSync(path.join(store, hash.slice(0, 2), hash))) throw new Error(`В хранилище нет куска ${hash}`);

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-chunks-"));
  const list = path.join(scratch, "files");
  fs.writeFileSync(list, Array.from(hashes.keys()).map(chunks.chunkPath).join("\n") + "\n");
  const remoteStore = `${args.target.replace(/\/+$/, "")}/${plan.chunkStore.remoteDir}/`;
  const remoteManifest = `${args.target.replace(/\/+$/, "")}/${artifact.remotePath}`;

  const pending = rsync(["--dry-run"], list, `${store}/`, remoteStore, args.sudo).split("\n").filter((line) => /^<f/.test(line)).length;
  process.stdout.write(`${plan.version}: ${hashes.size} кусков, на сервер уедет ${pending}; манифест -> ${artifact.remotePath}\n`);
  if (!args.publish) {
    process.stdout.write("Проверено. Выкладка — с --publish.\n");
    return;
  }

  rsync([], list, `${store}/`, remoteStore, args.sudo);
  const manifestDir = path.join(scratch, "manifest");
  fs.mkdirSync(manifestDir);
  fs.copyFileSync(manifestPath, path.join(manifestDir, path.basename(artifact.remotePath)));
  rsync([], null, `${manifestDir}/${path.basename(artifact.remotePath)}`, remoteManifest, args.sudo);

  // Проверка тем же кодом, что у клиента: манифест по адресу из канала и
  // выборка кусков (все новые, если их немного).
  const fetch = (url, options) => chunks.fetchBuffer(url, Object.assign({ isAllowedUrl: channel.isAllowedUrl }, options));
  const platform = manifest.platform;
  const url = endpoints.launcherFor(platform).chunkManifestUrls(path.basename(artifact.remotePath))[0];
  const remote = await fetch(url, { maxBytes: chunks.MAX_MANIFEST_BYTES });
  if (!remote.equals(body)) throw new Error(`Манифест на сервере отличается: ${url}`);
  const sample = Array.from(hashes.keys()).sort(() => Math.random() - 0.5).slice(0, SAMPLE);
  const base = endpoints.launcherFor(platform).chunkBaseUrls()[0];
  for (const hash of sample) {
    const { size, csize } = hashes.get(hash);
    const packed = await fetch(chunks.chunkUrl(base, hash), { maxBytes: csize });
    const data = chunks.decompress(packed, size);
    if (packed.length !== csize || chunks.sha256(data) !== hash) throw new Error(`Кусок на сервере повреждён: ${hash}`);
  }
  fs.rmSync(scratch, { recursive: true, force: true });
  process.stdout.write(`Выложено: ${pending} новых кусков, манифест ${url}; проверено ${sample.length} кусков по HTTPS.\n` +
    "Теперь можно публиковать установщик и канал.\n");
}

main().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
