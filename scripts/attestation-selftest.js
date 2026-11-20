"use strict";

// Как в официальной сборке: эти значения подставляет webpack (src/main/security/official.js).
process.env.HLSR_API_HOST = process.env.HLSR_API_HOST || "api.hlsr.pro";
process.env.HLSR_DISCORD_APP_ID = process.env.HLSR_DISCORD_APP_ID || "100000000000000001";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");
const seal = require("../src/main/security/attest-seal");
const BuildKey = require("../src/main/security/build-key");
const BuildIdentity = require("../src/main/security/build-identity");
const BuildAttestation = require("../src/main/security/attestation");
const Service = require("../src/main/online-profile/OnlineProfileService");
const attestKeys = require("./lib/attest-keys");
const compile = require("./compile-main-bytecode");
const launcherChunks = require("./lib/launcher-chunks");
const asarTool = require("@electron/asar");

const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
const CHUNK = 64 * 1024;
const sha = (data) => crypto.createHash("sha256").update(data).digest("hex");
const b64u = (buffer) => Buffer.from(buffer).toString("base64url");

function pair() {
  const seed = crypto.randomBytes(32);
  const publicKey = crypto.createPublicKey(crypto.createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, seed]), format: "der", type: "pkcs8" }));
  return { seed, sealed: seal.seal(seed), publicKey, publicHex: publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("hex") };
}

function fixture(t, { packaged = true } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-attest-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const asar = crypto.randomBytes(CHUNK * 5 + 777);
  fs.writeFileSync(path.join(root, "app.asar"), asar);
  const identity = new BuildIdentity({ version: "0.4.5", platform: "linux", packaged, resourcesPath: root });
  return { root, asar, identity, file: path.join(root, "app.asar") };
}

const table = (asar) => {
  const rows = [];
  for (let offset = 0; offset < asar.length; offset += CHUNK) rows.push([offset, Math.min(CHUNK, asar.length - offset)]);
  return rows;
};

// Сервер-двойник: тот же протокол, что у BuildAttestation.php (ключ — Ed25519
// публичный, вызов и билет — HMAC с секретом сервера).
function fakeServer({ asar, publicKey, header, now = () => Date.now(), status }) {
  const secret = crypto.randomBytes(32);
  const mac = (kind, body) => b64u(crypto.createHmac("sha256", secret).update(`${kind}.${body}`).digest());
  const sign = (kind, data) => { const body = b64u(JSON.stringify(Object.assign({ k: kind }, data))); return `${body}.${mac(kind, body)}`; };
  const read = (token, kind) => {
    const [body, tag] = token.split(".");
    if (tag !== mac(kind, body)) return null;
    return JSON.parse(Buffer.from(body, "base64url").toString("utf8"));
  };
  const rows = table(asar);
  const log = [];
  const request = async (url, options) => {
    const endpoint = new URL(url).pathname;
    const body = JSON.parse(options.body || "{}");
    log.push({ endpoint, headers: options.headers });
    if (status) return { status: status.code, body: JSON.stringify({ error: status.error }), headers: {} };
    if (options.headers["x-hlsr-build"] !== header) return { status: 403, body: JSON.stringify({ error: "CLIENT_MODIFIED" }), headers: {} };
    if (endpoint === "/v1/attest/challenge") {
      const picked = [0, 2, 4, 5];
      const token = sign("c", { i: picked, n: crypto.randomBytes(4).toString("hex"), e: Math.floor(now() / 1000) + 90 });
      return { status: 200, body: JSON.stringify({ challenge: token, ranges: picked.map((index) => rows[index]) }), headers: {} };
    }
    const data = read(body.challenge, "c");
    if (!data) return { status: 403, body: JSON.stringify({ error: "CLIENT_MODIFIED" }), headers: {} };
    const hashes = data.i.map((index) => sha(asar.subarray(rows[index][0], rows[index][0] + rows[index][1])));
    const message = ["hlsr-attest/1", header, body.challenge, hashes.join(",")].join("\n");
    if (!crypto.verify(null, Buffer.from(message), publicKey, Buffer.from(body.signature, "base64url")))
      return { status: 403, body: JSON.stringify({ error: "CLIENT_MODIFIED" }), headers: {} };
    return { status: 200, body: JSON.stringify({ ticket: sign("t", { e: Math.floor(now() / 1000) + 900 }), ttl: 900 }), headers: {} };
  };
  return { request, log };
}

test("запись ключа сборки не содержит seed и восстанавливается только целой", () => {
  const { seed, sealed } = pair();
  assert.ok(!sealed.includes(seed.toString("hex")) && !sealed.includes(seed.toString("base64")));
  assert.deepEqual(seal.unseal(sealed), seed);
  assert.notEqual(seal.seal(seed), sealed, "соль и «перец» случайны");
  for (const broken of ["", "x", sealed.replace(/^1\./, "2."), sealed.split(".").slice(0, 3).join("."), sealed.slice(0, -4)])
    assert.equal(seal.unseal(broken), null, broken);
  assert.equal(seal.unseal(undefined), null);
  assert.throws(() => seal.seal(Buffer.alloc(31)), /32 байта/);
});

test("ключ сборки подписывает Ed25519, а без записи недоступен", () => {
  const { sealed, publicKey } = pair();
  const key = new BuildKey(sealed);
  assert.equal(key.available(), true);
  const signature = key.sign("hello");
  assert.equal(signature.length, 64);
  assert.equal(crypto.verify(null, Buffer.from("hello"), publicKey, signature), true);
  assert.equal(crypto.verify(null, Buffer.from("hellO"), publicKey, signature), false);
  for (const empty of [new BuildKey(""), new BuildKey("мусор"), new BuildKey(undefined)]) {
    assert.equal(empty.available(), false);
    assert.throws(() => empty.sign("x"), /ATTEST_NO_KEY/);
  }
});

test("BuildIdentity отдаёт участки app.asar и не отвечает по подменённому файлу", async (t) => {
  const { asar, identity, file } = fixture(t);
  assert.match(await identity.header(), /^linux\/0\.4\.5\/[a-f0-9]{64}$/);
  assert.deepEqual(await identity.readRange(CHUNK, 1000), asar.subarray(CHUNK, CHUNK + 1000));
  assert.deepEqual(await identity.readRange(asar.length - 10, 10), asar.subarray(-10));
  for (const [offset, length] of [[-1, 10], [0, 0], [0, 256 * 1024 + 1], [1.5, 10], [0, NaN], ["0", 10]])
    await assert.rejects(identity.readRange(offset, length), /ASAR_BAD_RANGE/, `${offset}:${length}`);
  await assert.rejects(identity.readRange(asar.length - 5, 10), /ASAR_SHORT/);
  // Обновление при открытом лаунчере: хеш уже посчитан по старому файлу.
  fs.appendFileSync(file, "новая версия");
  await assert.rejects(identity.readRange(0, 10), (error) => error.code === "ASAR_CHANGED");
});

test("без app.asar (dev) участков нет", async (t) => {
  const { identity } = fixture(t, { packaged: false });
  assert.equal((await identity.header()).endsWith("/dev"), true);
  await assert.rejects(identity.readRange(0, 10), /ASAR_UNREADABLE/);
});

test("клиент получает билет и не ходит за ним на каждый запрос", async (t) => {
  const { asar, identity } = fixture(t);
  const keys = pair();
  const header = await identity.header();
  let clock = 1_800_000_000_000;
  const server = fakeServer({ asar, publicKey: keys.publicKey, header, now: () => clock });
  const attest = new BuildAttestation({ identity, key: new BuildKey(keys.sealed), request: server.request, now: () => clock });
  const ticket = await attest.ticket();
  assert.match(ticket, /^[\w-]+\.[\w-]+$/);
  assert.equal(await attest.ticket(), ticket);
  assert.equal(await attest.ticket(), ticket);
  assert.deepEqual(server.log.map((row) => row.endpoint), ["/v1/attest/challenge", "/v1/attest/prove"]);
  assert.equal(server.log[0].headers["x-hlsr-build"], header);
  // Билет обновляется заранее, пока он ещё действует.
  clock += (900 - 60) * 1000;
  const renewed = await attest.ticket();
  assert.notEqual(renewed, ticket);
  assert.equal(server.log.length, 4);
  // force берёт новый билет, но не чаще раза в 10 секунд.
  clock += 11_000;
  const forced = await attest.ticket(true);
  assert.notEqual(forced, renewed);
  assert.equal(server.log.length, 6);
});

test("параллельные запросы делят одно рукопожатие", async (t) => {
  const { asar, identity } = fixture(t);
  const keys = pair();
  const server = fakeServer({ asar, publicKey: keys.publicKey, header: await identity.header() });
  const attest = new BuildAttestation({ identity, key: new BuildKey(keys.sealed), request: server.request });
  const tickets = await Promise.all([attest.ticket(), attest.ticket(), attest.ticket()]);
  assert.equal(new Set(tickets).size, 1);
  assert.equal(server.log.length, 2);
});

test("подделанный app.asar или чужой ключ не получают билет и признаются изменёнными", async (t) => {
  const { asar, identity, file } = fixture(t);
  const keys = pair();
  const header = await identity.header();
  const server = fakeServer({ asar, publicKey: keys.publicKey, header });
  // Хеш в заголовке остался прежним (подделан), а байты на диске другие.
  const forged = Buffer.from(asar);
  forged[CHUNK * 2 + 5] ^= 0xff;
  fs.writeFileSync(file, forged);
  const tampered = new BuildIdentity({ version: "0.4.5", platform: "linux", packaged: true, resourcesPath: path.dirname(file) });
  tampered.header = () => Promise.resolve(header);
  tampered.pending = Promise.resolve(header);
  tampered.seen = BuildIdentity.fingerprint(fs.statSync(file));
  await assert.rejects(new BuildAttestation({ identity: tampered, key: new BuildKey(keys.sealed), request: server.request }).ticket(),
    (error) => error.code === "CLIENT_MODIFIED");
  fs.writeFileSync(file, asar);
  const restored = new BuildIdentity({ version: "0.4.5", platform: "linux", packaged: true, resourcesPath: path.dirname(file) });
  assert.equal(await restored.header(), header, "файл восстановлен байт в байт");
  const other = pair();
  const stranger = new BuildAttestation({ identity: restored, key: new BuildKey(other.sealed), request: server.request });
  await assert.rejects(stranger.ticket(), (error) => error.code === "CLIENT_MODIFIED");
});

test("сервер без аттестации даёт 404: клиент замолкает на час", async (t) => {
  const { asar, identity } = fixture(t);
  const keys = pair();
  let clock = 1_800_000_000_000;
  const server = fakeServer({ asar, publicKey: keys.publicKey, header: await identity.header(), status: { code: 404, error: "NOT_FOUND" }, now: () => clock });
  const attest = new BuildAttestation({ identity, key: new BuildKey(keys.sealed), request: server.request, now: () => clock });
  assert.equal(await attest.ticket(), null);
  assert.equal(await attest.ticket(true), null);
  assert.equal(server.log.length, 1);
  clock += 60 * 60 * 1000 + 1;
  assert.equal(await attest.ticket(), null);
  assert.equal(server.log.length, 2);
});

test("сбой сети и мусорный вызов не ломают запросы, а после сбоя клиент ждёт", async (t) => {
  const { identity } = fixture(t);
  const keys = pair();
  let clock = 1_800_000_000_000;
  const log = [];
  const request = async (url) => { log.push(url); throw new Error("ECONNRESET"); };
  const attest = new BuildAttestation({ identity, key: new BuildKey(keys.sealed), request, now: () => clock });
  assert.equal(await attest.ticket(), null);
  assert.equal(await attest.ticket(), null);
  assert.equal(log.length, 1, "30 секунд после сбоя к серверу не ходим");
  clock += 31_000;
  assert.equal(await attest.ticket(), null);
  assert.equal(log.length, 2);

  const bad = (challenge) => async () => ({ status: 200, body: JSON.stringify(challenge), headers: {} });
  const signed = [];
  const key = { available: () => true, sign: (message) => { signed.push(message); return Buffer.alloc(64); } };
  for (const challenge of [
    { challenge: "x", ranges: [] },
    { challenge: "x", ranges: [[0, 256 * 1024 + 1]] },
    { challenge: "x", ranges: [[-1, 10]] },
    { challenge: "x", ranges: Array.from({ length: 9 }, () => [0, 10]) },
    { challenge: 5, ranges: [[0, 10]] },
    { challenge: "x".repeat(2000), ranges: [[0, 10]] },
    { challenge: "x" },
  ]) {
    clock += 40_000;
    const attempt = new BuildAttestation({ identity, key, request: bad(challenge), now: () => clock });
    assert.equal(await attempt.ticket(), null);
  }
  assert.deepEqual(signed, [], "по недопустимому вызову ничего не подписывается");
});

test("без ключа сборки или не из упакованного лаунчера аттестация выключена", async (t) => {
  const { identity } = fixture(t);
  const dev = new BuildIdentity({ version: "0.4.5", platform: "linux", packaged: false, resourcesPath: "/nonexistent" });
  const request = async () => { throw new Error("запроса быть не должно"); };
  assert.equal(await new BuildAttestation({ identity, key: new BuildKey(""), request }).ticket(), null);
  assert.equal(await new BuildAttestation({ identity: dev, key: new BuildKey(pair().sealed), request }).ticket(true), null);
  assert.equal(new BuildAttestation({ identity, key: new BuildKey(pair().sealed), request }).enabled(), true);
});

test("онлайн-запрос с истёкшим билетом повторяется один раз с новым", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-attest-service-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const sent = [];
  const tickets = [];
  let answers = [{ status: 428, body: JSON.stringify({ error: "ATTEST_REQUIRED" }) }, { status: 200, body: JSON.stringify({ ok: true }) }];
  const service = new Service({ userDataDir: root, buildHeader: async () => "linux/0.4.5/" + "a".repeat(64),
    ticketProvider: async (force) => { tickets.push(force); return force ? "NEW" : "OLD"; },
    request: async (url, options) => { sent.push(options.headers); return answers.shift(); } });
  t.after(() => service.stop());
  assert.deepEqual(await service.call("/v1/link/start", {}, false), { ok: true });
  assert.deepEqual(tickets, [false, true]);
  assert.deepEqual(sent.map((headers) => headers["x-hlsr-ticket"]), ["OLD", "NEW"]);
  assert.equal(sent[1]["x-hlsr-build"], "linux/0.4.5/" + "a".repeat(64));

  // Новый билет не помог: понятная ошибка вместо цикла повторов.
  answers = [428, 428].map((status) => ({ status, body: JSON.stringify({ error: "ATTEST_REQUIRED" }) }));
  sent.length = 0;
  await assert.rejects(service.call("/v1/link/start", {}, false), (error) => error.code === "RUNNER_ATTEST_FAILED");
  assert.equal(sent.length, 2);

  // 428 без нашего кода (чужой прокси) повтором не лечится.
  answers = [{ status: 428, body: "{}" }];
  sent.length = 0;
  await assert.rejects(service.call("/v1/link/start", {}, false), (error) => error.code === "RUNNER_HTTP_428");
  assert.equal(sent.length, 1);
});

test("без сборки (dev) билет не запрашивается; CLIENT_MODIFIED при аттестации выходит из аккаунта", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-attest-service-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  let asked = 0;
  let modified = 0;
  const service = new Service({ userDataDir: root, buildHeader: async () => null, ticketProvider: async () => { asked++; return "T"; },
    onClientModified: () => modified++, request: async () => ({ status: 200, body: "{}" }) });
  t.after(() => service.stop());
  await service.call("/v1/link/start", {}, false);
  assert.equal(asked, 0);

  const refusing = new Service({ userDataDir: root, buildHeader: async () => "linux/0.4.5/" + "a".repeat(64), onClientModified: () => modified++,
    ticketProvider: async () => { throw Object.assign(new Error("CLIENT_MODIFIED"), { code: "CLIENT_MODIFIED" }); },
    request: async () => { throw new Error("запроса быть не должно"); } });
  t.after(() => refusing.stop());
  await assert.rejects(refusing.call("/v1/link/start", {}, false), (error) => error.code === "RUNNER_CLIENT_MODIFIED");
  assert.equal(modified, 1);
});

test("ключи выпуска: один на версию, права 0600, маркер для app.asar без seed", (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-attest-keys-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const env = { [attestKeys.DIR_ENV]: dir };
  const first = attestKeys.forBuild({ platform: "linux", version: "0.4.5", env });
  const again = attestKeys.forBuild({ platform: "linux", version: "0.4.5", env });
  assert.equal(first.marker.publicKey, again.marker.publicKey, "пересборка версии не меняет ключ");
  assert.notEqual(attestKeys.forBuild({ platform: "linux", version: "0.4.6", env }).marker.publicKey, first.marker.publicKey);
  assert.notEqual(attestKeys.forBuild({ platform: "win32", version: "0.4.5", env }).marker.publicKey, first.marker.publicKey);
  const record = JSON.parse(fs.readFileSync(attestKeys.fileFor("linux", "0.4.5", env), "utf8"));
  // На Windows права POSIX не хранятся: stat всегда даёт 0o666.
  if (process.platform !== "win32") assert.equal(fs.statSync(attestKeys.fileFor("linux", "0.4.5", env)).mode & 0o777, 0o600);
  assert.ok(!JSON.stringify(first.marker).includes(record.seed));
  const key = new BuildKey(first.seal);
  assert.equal(key.available(), true, "запись из сборки открывается рантаймом");
  const signature = key.sign("x");
  const publicKey = crypto.createPublicKey({ key: Buffer.concat([SPKI_PREFIX, Buffer.from(first.marker.publicKey, "hex")]), format: "der", type: "spki" });
  assert.equal(crypto.verify(null, Buffer.from("x"), publicKey, signature), true, "маркер хранит публичную половину того же ключа");
  assert.deepEqual(attestKeys.markerProblems(first.marker, { platform: "linux", version: "0.4.5" }), []);
  assert.match(attestKeys.markerProblems(first.marker, { platform: "win32", version: "0.4.5" }).join(), /ожидалась win32/);
  assert.match(attestKeys.markerProblems(first.marker, { platform: "linux", version: "0.4.6" }).join(), /ожидалась 0\.4\.6/);
  assert.throws(() => attestKeys.fileFor("darwin", "0.4.5", env), /платформа/);
  assert.throws(() => attestKeys.fileFor("linux", "../0.4.5", env), /версия/);
  assert.throws(() => attestKeys.keysDir({ [attestKeys.DIR_ENV]: path.join(__dirname, "..", "keys") }), /нельзя хранить/);
  fs.writeFileSync(attestKeys.fileFor("linux", "0.4.5", env), JSON.stringify({ platform: "linux", version: "0.4.5" }));
  assert.throws(() => attestKeys.loadOrCreate({ platform: "linux", version: "0.4.5", env }), /повреждён/);
});

test("загрузчик байткода не путается с обычным бандлом", () => {
  assert.equal(compile.isLoader(compile.loaderSource()), true);
  assert.equal(compile.isLoader("// hlsr-bytecode-loader\n" + "x".repeat(5000)), false);
  assert.equal(compile.isLoader('"use strict";\nrequire("electron");\n'), false);
  assert.match(compile.loaderSource(), /require\("\.\/main\.jsc"\)/);
});

// app.asar с маркером, как его кладёт webpack-сборка main.
async function asarWith(dir, marker) {
  const src = path.join(dir, "app-src");
  fs.mkdirSync(path.join(src, "dist", "electron"), { recursive: true });
  fs.writeFileSync(path.join(src, "package.json"), "{}");
  if (marker !== undefined) fs.writeFileSync(path.join(src, "dist", "electron", "attest.json"), typeof marker === "string" ? marker : JSON.stringify(marker));
  const tree = path.join(dir, "tree");
  fs.mkdirSync(path.join(tree, "resources"), { recursive: true });
  await asarTool.createPackage(src, path.join(tree, "resources", "app.asar"));
  return tree;
}

test("выпуск кладёт публичный ключ из app.asar в манифест чанков и не пускает сборку без ключа", async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-attest-release-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const where = { version: "0.4.5", platform: "linux" };
  const marker = { v: 1, platform: "linux", version: "0.4.5", publicKey: pair().publicHex };
  const withKey = await asarWith(path.join(dir, "a"), marker);
  const manifest = {};
  launcherChunks.attachAttestation(manifest, withKey, where);
  assert.deepEqual(manifest.attest, { v: 1, publicKey: marker.publicKey });

  const without = await asarWith(path.join(dir, "b"), undefined);
  assert.throws(() => launcherChunks.attachAttestation({}, without, where), /нет dist\/electron\/attest\.json/);
  const saved = process.env.HLSR_ATTEST_DISABLE;
  process.env.HLSR_ATTEST_DISABLE = "1";
  try {
    const open = {};
    launcherChunks.attachAttestation(open, without, where);
    assert.equal(open.attest, undefined, "осознанный выпуск без аттестации");
  } finally {
    if (saved === undefined) delete process.env.HLSR_ATTEST_DISABLE; else process.env.HLSR_ATTEST_DISABLE = saved;
  }

  for (const [name, bad, pattern] of [
    ["версия", { ...marker, version: "0.4.6" }, /ожидалась 0\.4\.5/],
    ["платформа", { ...marker, platform: "win32" }, /ожидалась linux/],
    ["ключ", { ...marker, publicKey: "zz" }, /публичный ключ/],
    ["схема", { v: 2 }, /не распознан/],
  ]) {
    const tree = await asarWith(path.join(dir, name), bad);
    assert.throws(() => launcherChunks.attachAttestation({}, tree, where), pattern, name);
  }
  // Не asar вообще (повреждённая сборка) — тоже отказ, а не тихий пропуск.
  fs.mkdirSync(path.join(dir, "c", "resources"), { recursive: true });
  fs.writeFileSync(path.join(dir, "c", "resources", "app.asar"), "не asar");
  assert.throws(() => launcherChunks.attachAttestation({}, path.join(dir, "c"), where), /нет dist/);
});
