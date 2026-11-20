#!/usr/bin/env node
"use strict";

const fs = require("fs");
const http = require("http");
const os = require("os");
const path = require("path");

const consent = require("../src/main/telemetry/consent");
const identity = require("../src/main/telemetry/identity");
const installReceipt = require("../src/main/telemetry/install-receipt");
const TelemetryService = require("../src/main/telemetry/TelemetryService");
const OnlineClient = require("../src/main/telemetry/online-client");
const TelemetryQueue = require("../src/main/telemetry/Queue");
const telemetryConfig = require("../src/main/telemetry/config");

const QA_ROOT_RE = /^[A-Za-z]:\\HLSR-TELEMETRY-QA-\d{8}-\d{6}$/;

const KEEP = process.argv.includes("--keep");
const silent = { info() {}, warn() {}, error() {} };

const results = [];
let failures = 0;

function check(name, condition, detail) {
  const ok = condition === true;
  if (!ok) failures++;
  results.push({ name, ok, detail: detail || "" });
  process.stdout.write(`  ${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}\n`);
}

async function settle(sender, timeoutMs) {
  const deadline = Date.now() + (timeoutMs || 5000);

  while (sender.inFlight && Date.now() < deadline)
    await new Promise((resolve) => setTimeout(resolve, 20));

  await new Promise((resolve) => setTimeout(resolve, 20));
}

function stamp() {
  const now = new Date();
  const pad = (value) => String(value).padStart(2, "0");
  return (
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-` +
    `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  );
}

function startServer() {
  const state = {
    batches: [],
    onlineRequests: [],
    mode: "ok",
    retryAfter: 60,
    online: {
      schemaVersion: 1,
      generatedAt: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
      ttlSeconds: 60,
      launcherOnline: 0,
      games: { "ghosting-renewed": { online: 0, activeRuns: 0 } },
    },
  };

  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));

    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      const url = req.url || "";

      if (url.startsWith("/v1/public/online")) {
        state.onlineRequests.push({ url, headers: Object.assign({}, req.headers) });

        const payload = JSON.stringify(state.online);
        res.writeHead(200, {
          "content-type": "application/json",
          etag: '"qa-online"',
          "cache-control": "public, max-age=60",
        });
        res.end(payload);
        return;
      }

      if (url.startsWith("/v1/telemetry/batch")) {
        let parsed = null;
        try {
          parsed = JSON.parse(body);
        } catch (error) {
          parsed = null;
        }

        state.batches.push({
          headers: Object.assign({}, req.headers),
          events: parsed && Array.isArray(parsed.events) ? parsed.events : [],
          bytes: Buffer.byteLength(body, "utf8"),
        });

        if (state.mode === "rate-limited") {
          res.writeHead(429, {
            "content-type": "application/json",
            "retry-after": String(state.retryAfter),
          });
          res.end('{"error":"RATE_LIMITED"}');
          return;
        }

        if (state.mode === "server-error") {
          res.writeHead(503, { "content-type": "application/json" });
          res.end('{"error":"INGEST_DISABLED"}');
          return;
        }

        const accepted = parsed && Array.isArray(parsed.events) ? parsed.events.length : 0;
        state.online.launcherOnline = 1;
        state.online.games["ghosting-renewed"].online = 1;

        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ schemaVersion: 1, accepted, duplicates: 0, rejected: 0 }));
        return;
      }

      res.writeHead(404, { "content-type": "application/json" });
      res.end('{"error":"NOT_FOUND"}');
    });
  });

  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      const port = server.address().port;
      resolve({ server, state, port });
    });
  });
}

async function main() {
  const root = path.join("C:" + path.sep, `HLSR-TELEMETRY-QA-${stamp()}`);

  if (!QA_ROOT_RE.test(root)) {
    process.stderr.write(`QA: имя корня не соответствует шаблону: ${root}\n`);
    process.exit(2);
  }
  if (fs.existsSync(root)) {
    process.stderr.write(`QA: корень уже существует: ${root}\n`);
    process.exit(2);
  }

  const userData = path.join(root, "userData");
  const installDir = path.join(root, "app");
  fs.mkdirSync(userData, { recursive: true });
  fs.mkdirSync(installDir, { recursive: true });

  process.stdout.write(`QA-корень: ${root}\n\n`);

  const realLibrary = path.join("C:" + path.sep, "HLSR");
  const realUserData = process.env.APPDATA ? path.join(process.env.APPDATA, "HLSR") : null;

  const before = {
    library: countEntries(realLibrary),
    userData: countEntries(realUserData),
  };

  const { server, state, port } = await startServer();
  const batchEndpoint = `http://127.0.0.1:${port}/v1/telemetry/batch`;
  const onlineEndpoint = `http://127.0.0.1:${port}/v1/public/online`;

  const queueFile = path.join(userData, telemetryConfig.QUEUE.fileName);

  function makeService(extra) {
    return new TelemetryService(
      Object.assign(
        {
          userDataDir: userData,
          launcherVersion: "0.3.0",
          backendConfigured: true,
          endpoint: batchEndpoint,
          logger: silent,
          localeProvider: () => "ru",
          isWindowVisible: () => true,
          installDir,
        },
        extra || {}
      )
    );
  }

  try {
    process.stdout.write("1. Телеметрия выключена\n");
    consent.set(userData, false);

    let telemetry = makeService();
    telemetry.start();
    telemetry.emit("launcher_session_started", {});
    telemetry.beginGameSession("ghosting-renewed", { gameVersion: "1.0.1", buildId: "qa-build" });
    telemetry.confirmGameRunning("ghosting-renewed");
    telemetry.launcherHeartbeat();
    telemetry.gameHeartbeat();
    await telemetry.sender.flush();
    telemetry.shutdown();

    check("ни одного POST телеметрии", state.batches.length === 0, `батчей: ${state.batches.length}`);
    check("идентификатор не создан", identity.read(userData) === null);
    check("очередь не создана", !fs.existsSync(queueFile));

    process.stdout.write("\n2. Телеметрия включена\n");
    consent.set(userData, true);

    fs.writeFileSync(
      path.join(installDir, installReceipt.MARKER_NAME),
      JSON.stringify({ schemaVersion: 1, appId: "{QA}", installId: "{11111111-2222-3333-4444-555555555555}" }),
      "utf8"
    );

    telemetry = makeService();
    telemetry.start();

    check("идентификатор создан", identity.read(userData) !== null);
    check("очередь создана", fs.existsSync(queueFile));

    const queuedTypes = readQueueTypes(queueFile);
    await settle(telemetry.sender);
    check(
      "установка подтверждена ровно один раз",
      queuedTypes.filter((type) => type === "launcher_install_completed").length === 1,
      queuedTypes.join(", ")
    );
    check("сессия лаунчера открыта", queuedTypes.includes("launcher_session_started"));

    telemetry.beginGameSession("ghosting-renewed", { gameVersion: "1.0.1", buildId: "qa-build" });
    telemetry.confirmGameRunning("ghosting-renewed");
    telemetry.launcherHeartbeat();
    telemetry.gameHeartbeat();
    telemetry.endGameSession("ghosting-renewed", { exitReason: "normal" });

    await settle(telemetry.sender);
    const flushed = await telemetry.sender.flush();
    await settle(telemetry.sender);

    check("батч доставлен", state.batches.length > 0, `батчей: ${state.batches.length}`);
    check(
      "очередь очищена после подтверждения",
      telemetry.queue.size() === 0,
      `осталось ${telemetry.queue.size()}`
    );

    const batch = state.batches[state.batches.length - 1] || { bytes: 0, events: [], headers: {} };
    check("батч не превышает 64 КиБ", batch.bytes <= 64 * 1024, `${batch.bytes} байт`);
    check("батч не превышает 50 событий", batch.events.length <= 50, `${batch.events.length}`);
    check(
      "Content-Type строго application/json",
      String(batch.headers["content-type"] || "").startsWith("application/json")
    );

    process.stdout.write("\n3. Содержимое событий\n");
    const allEvents = state.batches.flatMap((item) => item.events);
    const serialized = JSON.stringify(allEvents);

    check(
      "нет путей файловой системы",
      !/[A-Za-z]:\\\\|\/home\/|\/Users\//.test(serialized)
    );
    check("нет имени пользователя Windows", !containsUser(serialized));
    check("нет имени компьютера", !serialized.toLowerCase().includes(os.hostname().toLowerCase()));
    check("нет Steam ID", !/7656119\d{10}/.test(serialized));
    check(
      "все поля из схемы",
      allEvents.every((event) => require("../src/main/telemetry/event-schema").validate(event).ok)
    );

    const withInstall = allEvents.find((event) => event.type === "launcher_install_completed");
    check("у события установки sessionId = null", withInstall && withInstall.sessionId === null);

    process.stdout.write("\n4. Ограничение частоты\n");
    state.mode = "rate-limited";
    state.retryAfter = 120;

    telemetry.emit("launcher_heartbeat", {});
    await settle(telemetry.sender);
    const limited = await telemetry.sender.flush();

    check("429 распознан", limited.reason === "rate-limited", `реакция: ${limited.reason || limited.skipped}`);
    check("события сохранены", telemetry.queue.size() > 0, `в очереди ${telemetry.queue.size()}`);
    check(
      "Retry-After уважается",
      limited.retryAfterMs === 120000,
      `пауза ${limited.retryAfterMs} мс`
    );

    const skipped = await telemetry.sender.flush();
    check("до истечения паузы повтора нет", skipped.skipped === "backoff");

    state.mode = "ok";
    telemetry.sender.nextAttemptAt = 0;
    await telemetry.sender.flush();
    await settle(telemetry.sender);
    check("после паузы очередь уходит", telemetry.queue.size() === 0, `осталось ${telemetry.queue.size()}`);

    process.stdout.write("\n5. Отключение телеметрии посреди работы\n");
    telemetry.emit("launcher_heartbeat", {});
    check("в очереди есть событие", telemetry.queue.size() > 0);

    const batchesBefore = state.batches.length;
    telemetry.setEnabled(false);
    await settle(telemetry.sender);

    check("очередь удалена немедленно", !fs.existsSync(queueFile));

    telemetry.launcherHeartbeat();
    telemetry.emit("launcher_heartbeat", {});
    await telemetry.sender.flush();
    await settle(telemetry.sender);

    check(
      "после отключения новых запросов нет",
      state.batches.length === batchesBefore,
      `было ${batchesBefore}, стало ${state.batches.length}`
    );
    check("очередь не воссоздана", !fs.existsSync(queueFile));

    process.stdout.write("\n6. Повреждённая очередь\n");
    consent.set(userData, true);
    fs.writeFileSync(queueFile, "мусор\nещё мусор\n{{{\n", "utf8");

    const queue = new TelemetryQueue({ file: queueFile, logger: silent });
    const read = queue.readAll();

    check("повреждённая очередь не читается как события", read.events.length === 0);
    check("файл убран в карантин", !fs.existsSync(queueFile));
    check(
      "карантинный файл сохранён для разбора",
      fs.readdirSync(userData).some((name) => name.includes(".corrupt-"))
    );

    process.stdout.write("\n7. Публичный счётчик онлайна\n");
    const online = new OnlineClient({
      endpoint: onlineEndpoint,
      launcherVersion: "0.3.0",
      logger: silent,
      backendConfigured: true,
      isWindowVisible: () => true,
    });

    await online.fetchOnce();
    const snapshot = online.snapshot();

    check("счётчик получен", snapshot.status === "ok", `статус ${snapshot.status}`);
    check("число разобрано", typeof snapshot.launcherOnline === "number");

    const onlineRequest = state.onlineRequests[state.onlineRequests.length - 1];
    check("в запросе нет параметров", !onlineRequest.url.includes("?"), onlineRequest.url);
    check("в запросе нет cookie", onlineRequest.headers.cookie === undefined);
    check(
      "в заголовках нет идентификаторов",
      !JSON.stringify(onlineRequest.headers).match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}/i)
    );

    const hidden = new OnlineClient({
      endpoint: onlineEndpoint,
      logger: silent,
      backendConfigured: true,
      isWindowVisible: () => false,
    });
    const hiddenResult = await hidden.fetchOnce();
    check("свёрнутый лаунчер не опрашивает счётчик", hiddenResult.skipped === "hidden");

    const broken = new OnlineClient({
      endpoint: `http://127.0.0.1:${port + 1}/v1/public/online`,
      logger: silent,
      backendConfigured: true,
      isWindowVisible: () => true,
    });
    await broken.fetchOnce();
    check(
      "недоступный счётчик даёт «неизвестно», а не 0",
      broken.snapshot().status === "unknown" && broken.snapshot().launcherOnline === null
    );

    process.stdout.write("\n8. Сброс идентичности\n");
    telemetry = makeService();
    telemetry.start();
    await settle(telemetry.sender);

    const idBefore = identity.read(userData).installationId;
    telemetry.resetIdentity();
    await settle(telemetry.sender);
    const idAfter = identity.read(userData).installationId;

    check("идентификатор сменился", idBefore !== idAfter);
    check("очередь начата заново", telemetry.queue.size() <= 2, `в очереди ${telemetry.queue.size()}`);

    process.stdout.write("\n9. Событие удаления\n");
    consent.set(userData, true);
    telemetry = makeService();
    check("удаление ставится в очередь при согласии", telemetry.reportUninstallInitiated() === true);

    consent.set(userData, false);
    telemetry = makeService();
    check(
      "при выключенной телеметрии удаление не сообщается",
      telemetry.reportUninstallInitiated() === false
    );

    telemetry.stopTimers();
  } finally {
    server.close();
  }

  process.stdout.write("\n10. Изоляция\n");

  const after = {
    library: countEntries(realLibrary),
    userData: countEntries(realUserData),
  };

  check(
    "C:\\HLSR не изменён",
    before.library === after.library,
    `было ${before.library}, стало ${after.library}`
  );
  check(
    "боевой %APPDATA%\\HLSR не изменён",
    before.userData === after.userData,
    `было ${before.userData}, стало ${after.userData}`
  );

  const report = {
    generatedAt: new Date().toISOString(),
    qaRoot: root,
    node: process.version,
    passed: results.filter((item) => item.ok).length,
    failed: failures,
    checks: results,
  };

  const reportDir = path.resolve(__dirname, "..", "qa-out");
  fs.mkdirSync(reportDir, { recursive: true });
  fs.writeFileSync(
    path.join(reportDir, "qa-telemetry-report.json"),
    JSON.stringify(report, null, 2) + "\n",
    "utf8"
  );

  process.stdout.write(
    `\n${"-".repeat(60)}\nПройдено ${report.passed}, провалено ${failures}\n` +
      `Отчёт: qa-out/qa-telemetry-report.json\n`
  );

  if (KEEP) {
    process.stdout.write(`QA-корень оставлен: ${root}\n`);
  } else if (QA_ROOT_RE.test(root) && fs.existsSync(root)) {
    fs.rmSync(root, { recursive: true, force: true });
    process.stdout.write(`QA-корень удалён: ${root}\n`);
  } else {
    process.stdout.write(`QA-корень НЕ удалён (не прошёл проверку): ${root}\n`);
  }

  process.exit(failures === 0 ? 0 : 1);
}

function countEntries(dir) {
  if (!dir) return null;
  try {
    if (!fs.existsSync(dir)) return 0;
    return fs.readdirSync(dir).length;
  } catch (error) {
    return null;
  }
}

function readQueueTypes(file) {
  try {
    return fs
      .readFileSync(file, "utf8")
      .split("\n")
      .filter((line) => line.trim())
      .map((line) => JSON.parse(line).type);
  } catch (error) {
    return [];
  }
}

function containsUser(text) {
  const user = process.env.USERNAME || process.env.USER || "";
  return user.length > 2 && text.toLowerCase().includes(user.toLowerCase());
}

main().catch((error) => {
  process.stderr.write(`QA: сбой — ${error.stack || error.message}\n`);
  process.exit(2);
});
