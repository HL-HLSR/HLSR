#!/usr/bin/env node
"use strict";

// Копия scripts/qa-launcher-feed.js 0.4.7 (подпроект 6, задача 8) с правками: blockmap нет (Р4),
// установщик Linux (.AppImage) отдаётся и портится так же, как .exe. Локальная лента для QA-сборок
// (HLSR_UPDATE_FEED): только 127.0.0.1, имена без подкаталогов, Range для докачки.

const fs = require("fs");
const http = require("http");
const path = require("path");

const HOST = "127.0.0.1";

const TYPES = {
  ".exe": "application/octet-stream",
  ".AppImage": "application/octet-stream",
  ".yml": "text/yaml",
  ".json": "application/json",
};

const isInstaller = (name) => name.endsWith(".exe") || name.endsWith(".AppImage");

function parseArgs(argv) {
  const args = { faults: {} };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--dir") args.dir = argv[++i];
    else if (arg === "--port") args.port = Number(argv[++i]);
    else if (arg === "--channel") args.channel = argv[++i];
    else if (arg === "--break-installer") args.faults.installer = true;
    else if (arg === "--break-yml") args.faults.yml = true;
    else if (arg === "--no-channel") args.faults.noChannel = true;
    else if (arg === "--cut-download") args.faults.cut = true;
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }

  return args;
}

function usage() {
  return (
    "node tools/release/node/qa-launcher-feed.js --dir <каталог с артефактами> [--port N]\n" +
    "  [--channel <launcher-channel.json>] [--break-installer] [--break-yml]\n" +
    "  [--no-channel] [--cut-download]\n"
  );
}

function createServer(options) {
  const dir = path.resolve(options.dir);
  const faults = options.faults || {};
  const channelFile = options.channel ? path.resolve(options.channel) : null;

  const server = http.createServer((req, res) => {
    const name = decodeURIComponent(req.url.split("?")[0]).replace(/^\/+/, "");

    if (!name || name.indexOf("..") !== -1 || name.indexOf("/") !== -1) {
      res.writeHead(404).end("not found");
      return;
    }

    if (name === "launcher-channel.json") {
      if (faults.noChannel) {
        res.writeHead(404).end("not found");
        return;
      }

      const file = channelFile || path.join(dir, name);
      if (!fs.existsSync(file)) {
        res.writeHead(404).end("not found");
        return;
      }

      const body = fs.readFileSync(file);
      res.writeHead(200, { "content-type": "application/json", "content-length": body.length });
      res.end(body);
      return;
    }

    const file = path.join(dir, name);
    if (!fs.existsSync(file) || !fs.statSync(file).isFile()) {
      res.writeHead(404).end("not found");
      return;
    }

    let body = fs.readFileSync(file);

    if (faults.yml && name.startsWith("latest") && name.endsWith(".yml")) body = Buffer.concat([body, Buffer.from("\n# broken\n")]);
    if (faults.installer && isInstaller(name)) {
      body = Buffer.from(body);
      body[0] ^= 0xff;
    }

    const type = TYPES[path.extname(name)] || "application/octet-stream";
    const range = req.headers.range;

    if (range && /^bytes=\d*-\d*$/.test(range)) {
      const [rawStart, rawEnd] = range.replace("bytes=", "").split("-");
      const start = rawStart ? Number(rawStart) : 0;
      const end = rawEnd ? Number(rawEnd) : body.length - 1;
      const slice = body.subarray(start, end + 1);

      res.writeHead(206, {
        "content-type": type,
        "content-length": slice.length,
        "content-range": `bytes ${start}-${start + slice.length - 1}/${body.length}`,
        "accept-ranges": "bytes",
      });
      res.end(slice);
      return;
    }

    res.writeHead(200, {
      "content-type": type,
      "content-length": body.length,
      "accept-ranges": "bytes",
    });

    if (faults.cut && isInstaller(name)) {
      res.write(body.subarray(0, Math.floor(body.length / 3)));
      res.destroy();
      return;
    }

    res.end(body);
  });

  return server;
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help || !args.dir) {
    process.stdout.write(usage());
    return args.help ? 0 : 2;
  }

  const server = createServer(args);

  server.listen(args.port || 0, HOST, () => {
    const { port } = server.address();
    process.stdout.write(`FEED http://${HOST}:${port}/\n`);
    process.stdout.write(`dir=${path.resolve(args.dir)}\n`);
    if (Object.keys(args.faults).length)
      process.stdout.write(`faults=${Object.keys(args.faults).join(",")}\n`);
  });

  return 0;
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`feed не запущен: ${error.message}\n`);
    process.exitCode = 2;
  }
}

module.exports = { createServer };
