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
const Service = require("../src/main/online-profile/OnlineProfileService");
const SecureSession = require("../src/main/online-profile/SecureSession");
const qa = require("../src/main/online-profile/local-qa");
const token = "hlsr_" + "a".repeat(16) + "_" + "b".repeat(43);
const id = "76561198000000111";
const card = (extra) => Object.assign({ steamId64: id, personaName: "Fake runner", registeredAt: "2026-10-05T10:00:00Z", games: {}, details: {}, badges: ["verified"], generationId: "a".repeat(32) }, extra);
const reply = (body, status = 200) => ({ status, body: JSON.stringify(body), headers: {} });
function fixture(t, extra) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-profile-qa-test-"));
  const client = new Service(Object.assign({ userDataDir: root, accountProvider: () => ({ steamId64: id, personaName: "Fake runner" }) }, extra));
  t.after(() => { client.stop(); fs.rmSync(root, { recursive: true, force: true }); });
  return { root, client };
}

async function signedIn(t, request) {
  const { client } = fixture(t, { openExternal: async () => {}, request: async (url, options) => {
    if (url.endsWith("/start")) return reply({ deviceCode: "c".repeat(64), userCode: "ABCD-EFGH", verifyUrl: "https://api.hlsr.pro/link?code=ABCD-EFGH", expiresIn: 600, interval: 5 });
    if (url.endsWith("/poll")) return reply({ state: "ready", token, steamId64: id, profile: card() });
    return request ? request(url, options) : reply({ profile: card() });
  } });
  await client.beginLogin(false);
  await client.poll(client.pending);
  assert.equal(client.snapshot().account.signedIn, true);
  return client;
}

test("выход на всех устройствах зовёт сервер и убирает вход", async (t) => {
  const calls = [];
  const client = await signedIn(t, async (url) => { calls.push(url); return reply({ ok: true }); });
  const snapshot = await client.logoutAll();
  assert.ok(calls.some((url) => url.endsWith("/v2/runner/logout-all")));
  assert.equal(snapshot.account.signedIn, false);
  assert.equal(client.getAuthToken(), null);
});

test("если сервер не ответил, вход не убирается: другие устройства ещё в аккаунте", async (t) => {
  const client = await signedIn(t, async () => reply({ error: "UNAVAILABLE" }, 503));
  await assert.rejects(client.logoutAll());
  assert.equal(client.snapshot().account.signedIn, true);
  assert.notEqual(client.getAuthToken(), null);
});

test("a local Steam cache cannot authenticate an HLSR profile", (t) => {
  const { client } = fixture(t);
  assert.equal(client.snapshot().account.signedIn, false);
  assert.equal(client.getAuthToken(), null);
});

test("device login opens only the configured backend and never exposes bearer or device secrets to renderer", async (t) => {
  let opened, authenticated = 0;
  const { client } = fixture(t, { openExternal: async (url) => { opened = url; }, onAuthenticated: () => authenticated++,
    request: async (url) => {
      if (url.endsWith("/start")) return reply({ deviceCode: "c".repeat(64), userCode: "ABCD-EFGH", verifyUrl: "https://api.hlsr.pro/link?code=ABCD-EFGH", expiresIn: 900 });
      if (url.endsWith("/poll")) return reply({ state: "ready", token, steamId64: id, profile: card() });
      return reply({ profile: card() });
    } });
  const pending = await client.beginLogin(false);
  assert.match(opened, /^https:\/\/api\.hlsr\.pro\/link/);
  assert.equal(pending.userCode, "ABCD-EFGH");
  assert.ok(!JSON.stringify(pending).includes("c".repeat(64)));
  await client.poll(client.pending);
  assert.equal(authenticated, 1);
  assert.equal(client.snapshot().account.signedIn, true);
  assert.ok(!JSON.stringify(client.snapshot()).includes(token));
});

test("a forged login URL cannot receive a user or device secret", async (t) => {
  let opened = false;
  const { client } = fixture(t, { openExternal: async () => { opened = true; }, request: async () => reply({ deviceCode: "c".repeat(64), userCode: "ABCD-EFGH", verifyUrl: "https://evil.example/link" }) });
  await assert.rejects(client.beginLogin(false), /INVALID_LOGIN_URL/);
  assert.equal(opened, false);
});

test("no pre-update history is sent and the old migration file is removed", async (t) => {
  const roots = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-profile-qa-history-"));
  fs.writeFileSync(path.join(roots, "game-stats.json"), JSON.stringify({ games: { "hl1-won": { totalPlaySeconds: 3600, launchCount: 2 } } }));
  fs.writeFileSync(path.join(roots, "online-history-migration.json"), JSON.stringify({ version: 1, completed: {}, games: {} }));
  const urls = [];
  const request = async (url) => { urls.push(url); return reply({ profile: card() }); };
  const client = new Service({ userDataDir: roots, request, accountProvider: () => ({ steamId64: id }) });
  t.after(() => { client.stop(); fs.rmSync(roots, { recursive: true, force: true }); });
  assert.equal(fs.existsSync(path.join(roots, "online-history-migration.json")), false);
  client.token = token; client.profile = card(); await client.sync();
  assert.deepEqual(urls.filter((url) => url.endsWith("/migrate")), []);
  assert.equal(client.snapshot().stats.totalSeconds, 0);
  assert.equal("legacySeconds" in client.snapshot().stats, false);
});

test("a delayed unauthorized response cannot clear a newer login", async (t) => {
  let finish;
  const { client } = fixture(t, { request: () => new Promise((resolve) => { finish = resolve; }) });
  client.token = token; client.profile = card();
  const pending = client.call("/v2/runner/sync", {});
  const newer = token.slice(0, -1) + "z"; client.token = newer;
  // Запрос уходит после подсчёта хеша сборки — ждём, пока он будет отправлен.
  while (!finish) await new Promise(setImmediate);
  finish(reply({}, 401)); await assert.rejects(pending, /STALE_RESPONSE/);
  assert.equal(client.token, newer);
});

test("OS encryption is used when available, otherwise the session stays in an owner-only file", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-profile-qa-vault-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = path.join(root, "session.json");
  const key = crypto.randomBytes(32);
  const storage = { isEncryptionAvailable: () => true, getSelectedStorageBackend: () => "fake-test-vault",
    encryptString: (plain) => { const nonce = crypto.randomBytes(12); const cipher = crypto.createCipheriv("aes-256-gcm", key, nonce); return Buffer.concat([nonce, cipher.update(plain), cipher.final(), cipher.getAuthTag()]); },
    decryptString: (data) => { const decipher = crypto.createDecipheriv("aes-256-gcm", key, data.subarray(0, 12)); decipher.setAuthTag(data.subarray(-16)); return Buffer.concat([decipher.update(data.subarray(12, -16)), decipher.final()]).toString(); } };
  const vault = new SecureSession(file, storage); vault.write({ token });
  assert.ok(!fs.readFileSync(file, "utf8").includes(token)); assert.equal(vault.read().token, token);
  // The keyring went away after an encrypted write: the old session is unreadable, not a crash.
  assert.equal(new SecureSession(file, { isEncryptionAvailable: () => false }).read(), null);

  for (const keyless of [undefined, { isEncryptionAvailable: () => false },
    { isEncryptionAvailable: () => true, getSelectedStorageBackend: () => "basic_text" }]) {
    fs.writeFileSync(file + ".tmp", "stale", { mode: 0o644 });
    const plain = new SecureSession(file, keyless); plain.write({ token });
    assert.equal(plain.read().token, token);
    if (process.platform !== "win32") assert.equal(fs.statSync(file).mode & 0o777, 0o600);
    plain.clear(); assert.equal(fs.existsSync(file), false); assert.equal(plain.read(), null);
  }
});

test("a sign-in survives a launcher restart without a system keyring", async (t) => {
  const { root, client } = fixture(t);
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  assert.ok(client.profile); client.save();
  const restarted = new Service({ userDataDir: root, accountProvider: () => ({ steamId64: id, personaName: "Fake runner" }) });
  t.after(() => restarted.stop());
  assert.equal(restarted.getAuthToken(), token);
  assert.equal(restarted.snapshot().status, "signed-in");
});

test("local QA endpoints require isolated data and are rejected by packaged launchers", () => {
  // local-qa сверяет папку с os.tmpdir(): это /tmp только на Linux без TMPDIR.
  const isolated = path.join(os.tmpdir(), "hlsr-profile-qa-test");
  assert.equal(qa.settings({ isPackaged: true }, { HLSR_PROFILE_QA_ROOT: isolated }), null);
  assert.throws(() => qa.settings({ isPackaged: false }, { HLSR_PROFILE_QA_ROOT: "/home/user/real-profile" }));
  assert.throws(() => qa.settings({ isPackaged: false }, { HLSR_PROFILE_QA_ROOT: isolated, HLSR_PROFILE_QA_API: "https://evil.example" }));
  assert.equal(qa.settings({ isPackaged: false }, { HLSR_PROFILE_QA_ROOT: isolated }).apiBase, "http://127.0.0.1:18895");
});

test("the Steam avatar from the server fills in when the local Steam cache has none", () => {
  const profileState = require("../src/main/online-profile/profile-state");
  const profile = profileState.normalize(card({ details: { avatarUrl: "https://avatars.steamstatic.com/abc_full.jpg" } }));
  const local = { steamId64: id, personaName: "Fake runner" };
  assert.equal(profileState.state(profile, local, true).account.avatarDataUri, "https://avatars.steamstatic.com/abc_full.jpg");
  const cached = Object.assign({}, local, { avatarDataUri: "data:image/png;base64,AAAA" });
  assert.equal(profileState.state(profile, cached, true).account.avatarDataUri, "data:image/png;base64,AAAA");
});

test("скрытие SteamID приходит с сервера только как true/false, без флага его нет", () => {
  const profileState = require("../src/main/online-profile/profile-state");
  assert.equal(profileState.normalize(card({ details: { hideSteamId: true } })).details.hideSteamId, true);
  assert.equal(profileState.normalize(card({ details: { hideSteamId: false } })).details.hideSteamId, false);
  assert.equal("hideSteamId" in profileState.normalize(card({ details: {} })).details, false);
  assert.equal("hideSteamId" in profileState.normalize(card({ details: { hideSteamId: "true" } })).details, false);
  // Приватность в списках игроков: только явное true.
  assert.deepEqual(["hideFromPlayers", "hideGame"].map((key) => profileState.normalize(card({ details: { hideFromPlayers: true, hideGame: "true" } })).details[key]), [true, false]);
});

test("closing a game while a heartbeat is in flight still ends the server session", async (t) => {
  const calls = [];
  let release;
  const { client } = fixture(t, { request: (url) => {
    const route = url.replace(/^.*\/v2\/runner\//, "");
    calls.push(route);
    if (route === "session/heartbeat") return new Promise((resolve) => { release = () => resolve(reply({ profile: card() })); });
    return Promise.resolve(reply({ profile: card() }));
  } });
  client.token = token; client.profile = card();
  client.gameStart("ghosting-renewed");
  const session = client.active.get("ghosting-renewed");
  await new Promise((resolve) => setImmediate(resolve));
  const heartbeat = client.beat("ghosting-renewed", session, false);
  await new Promise((resolve) => setImmediate(resolve));
  client.gameEnd("ghosting-renewed");
  release(); await heartbeat;
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(calls, ["session/start", "session/heartbeat", "session/end"]);
  assert.equal(client.active.has("ghosting-renewed"), false);
});

test("run page links are learned ahead of a click and remembered across restarts", async (t) => {
  let lookups = 0;
  const request = async (url) => {
    if (url.startsWith("https://www.speedrun.com/api/v1/runs/")) {
      lookups++;
      const run = url.slice(-8);
      return reply({ data: { id: run, weblink: "https://www.speedrun.com/hl2hl1/runs/" + run } });
    }
    return reply({ profile: card({ details: { pinnedRun: { id: "yw7pd10z", seconds: 2629.85, url: "https://www.speedrun.com/runs/yw7pd10z" } } }), runs: [
      { id: "abcd1234", seconds: 10, url: "https://www.speedrun.com/runs/abcd1234" },
      { id: "yw7pd10z", seconds: 2629.85, url: "https://www.speedrun.com/runs/yw7pd10z" },
    ] });
  };
  const { root, client } = fixture(t, { request });
  client.token = token;
  client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  await client.action("speedrun/bests", {});
  await client.runLinkQueue;
  assert.equal(lookups, 2, "pinned and listed runs are resolved once each, in the background");
  assert.equal(await client.speedrunRunLink("yw7pd10z"), "https://www.speedrun.com/hl2hl1/runs/yw7pd10z");
  assert.equal(lookups, 2, "a click uses the remembered link without asking speedrun.com");

  const again = new Service({ userDataDir: root, request });
  t.after(() => again.stop());
  assert.equal(await again.speedrunRunLink("abcd1234"), "https://www.speedrun.com/hl2hl1/runs/abcd1234");
  assert.equal(lookups, 2, "the remembered links survive a restart");
});

test("обычные достижения видны закрытыми, секретные до получения — замок без названия, «Разминка» считает запуски", () => {
  const achievements = require("../src/main/profile/achievements");
  const games = { "hl1-won": { launchCount: 10, totalPlaySeconds: 60 }, "hl2-2707": { launchCount: 7, totalPlaySeconds: 60 } };
  const closed = achievements.evaluate({ games });
  assert.equal(closed.length, achievements.ACHIEVEMENTS.length, "в списке и счётчике все, секретные тоже");
  const wake = closed.find((item) => item.id === "wake-up");
  assert.deepEqual([wake.unlocked, wake.hidden, wake.title, wake.description], [false, true, "#L_ACH_SECRET_TITLE", "#L_ACH_SECRET_DESC"]);
  assert.equal(closed.find((item) => item.id === "scout").unlocked, false);
  const warmup = closed.find((item) => item.id === "warmup");
  assert.deepEqual([warmup.unlocked, warmup.value, warmup.goal], [false, 17, 25]);

  const open = achievements.evaluate({ games: { "hl1-won": { launchCount: 40 } }, unlocked: ["scout", "almost"] });
  assert.equal(open.find((item) => item.id === "scout").unlocked, true);
  const almost = open.find((item) => item.id === "almost");
  assert.deepEqual([almost.secret, almost.hidden, almost.title], [true, undefined, "#L_ACH_ALMOST_TITLE"]);
  assert.equal(open.find((item) => item.id === "wake-up").hidden, true);
  assert.equal(open.find((item) => item.id === "warmup").unlocked, true);
});

const NEW_IDS = ["patron", "king-of-the-hill", "double-shift", "cursed", "avid-runner", "mountain-dweller", "shut-it-down", "around-the-clock",
  "hard-worker", "day-after-day", "keep-the-flame", "piano", "frankenstein", "hoarder", "meet-the-maker", "impossible", "particle-accelerator",
  "respected", "good-memory", "hesoyam"];

test("достижения 0.4.9: все 20 по договору, редкость, секретность, часы и запуски по порогам", () => {
  const achievements = require("../src/main/profile/achievements");
  const byId = Object.fromEntries(achievements.ACHIEVEMENTS.map((item) => [item.id, item]));
  assert.equal(achievements.ACHIEVEMENTS.length, 38);
  assert.equal(byId["video-editor"].kind, "event", "«Монтажёр» — обычное, открывает main после экспорта клипа");
  assert.ok(!achievements.RENDERER_UNLOCK_IDS.includes("video-editor"));
  for (const id of NEW_IDS) assert.ok(byId[id], id);
  const tiers = (tier) => NEW_IDS.filter((id) => byId[id].tier === tier).sort();
  assert.deepEqual(tiers("rainbow"), ["king-of-the-hill"]);
  assert.deepEqual(tiers("gold"), ["avid-runner", "good-memory", "keep-the-flame", "particle-accelerator", "patron", "shut-it-down"]);
  assert.deepEqual(tiers("purple"), ["around-the-clock", "cursed", "day-after-day", "mountain-dweller"]);
  assert.deepEqual(achievements.SECRET_IDS.filter((id) => NEW_IDS.includes(id)).sort(), ["good-memory", "hesoyam", "impossible", "king-of-the-hill", "meet-the-maker", "piano"]);
  assert.deepEqual(achievements.SERVER_IDS.slice().sort(), ["king-of-the-hill", "particle-accelerator", "patron", "respected"]);
  for (const id of achievements.SERVER_IDS) assert.equal(achievements.UNLOCK_IDS.includes(id), false, id + " выдаёт только сервер");
  for (const id of ["good-memory", "hesoyam", "frankenstein", "hoarder", "piano"]) assert.ok(achievements.RENDERER_UNLOCK_IDS.includes(id), id);
  for (const id of ["meet-the-maker", "impossible", "around-the-clock", "hard-worker", "patron"]) assert.equal(achievements.RENDERER_UNLOCK_IDS.includes(id), false, id);
  for (const id of NEW_IDS) assert.equal(achievements.evaluate({}).find((item) => item.id === id).unlocked, false, id);

  const at = (hours, launches) => achievements.evaluate({ games: { "hl1-won": { totalPlaySeconds: hours * 3600, launchCount: launches } } });
  const state = (list, id) => { const item = list.find((entry) => entry.id === id); return [item.unlocked, item.value, item.goal]; };
  const mid = at(120, 600);
  assert.deepEqual(state(mid, "double-shift"), [true, 120, 100]);
  assert.deepEqual(state(mid, "cursed"), [false, 120, 500]);
  assert.deepEqual(state(mid, "mountain-dweller"), [true, 500, 500]);
  assert.deepEqual(state(mid, "shut-it-down"), [false, 600, 1000]);
  const top = at(1000, 1000);
  for (const id of ["double-shift", "cursed", "avid-runner", "mountain-dweller", "shut-it-down"]) assert.equal(top.find((item) => item.id === id).unlocked, true, id);
  assert.equal(top.find((item) => item.id === "avid-runner").tier, "gold");
});

test("серверные достижения: приходят в details.secrets и видны, лаунчер их не открывает", async (t) => {
  const profileState = require("../src/main/online-profile/profile-state");
  const profile = profileState.normalize(card({ details: { secrets: ["patron", "respected", "made-up", "scout"] } }));
  assert.deepEqual(profile.details.secrets, ["patron", "respected", "scout"]);
  const list = profileState.state(profile, {}, true).achievements;
  assert.equal(list.find((item) => item.id === "patron").unlocked, true);
  assert.equal(list.some((item) => item.id === "king-of-the-hill"), false, "неполученный «Царь горы» — замок без id");
  const { client } = fixture(t, { request: async () => reply({ profile: card() }) });
  client.token = token; client.profile = profile;
  for (const id of ["patron", "king-of-the-hill", "particle-accelerator", "respected"]) {
    await assert.rejects(client.unlock(id), /RUNNER_UNKNOWN_SECRET/, id);
    await assert.rejects(client.unlockFromRenderer(id), /RUNNER_UNKNOWN_SECRET/, id);
  }
  assert.deepEqual(client.secrets, []);
});

test("порядок в профиле: полученные радужное → золото → фиолет → обычные, потом неполученные, внизу секретные под замком", () => {
  const profileState = require("../src/main/online-profile/profile-state");
  const profile = profileState.normalize(card({ games: { "hl1-won": { totalPlaySeconds: 600 * 3600, launchCount: 30 } },
    details: { secrets: ["scout", "patron", "king-of-the-hill", "around-the-clock", "almost"] } }));
  const list = profileState.state(profile, {}, true).achievements;
  const group = (item) => (item.unlocked ? ({ rainbow: 0, gold: 1, purple: 2 }[item.tier] ?? 3) : item.hidden ? 5 : 4);
  assert.deepEqual(list.map(group), list.map(group).slice().sort((a, b) => a - b), "группы идут по порядку");
  assert.equal(list[0].id, "king-of-the-hill");
  assert.equal(list[1].id, "patron");
  assert.deepEqual(list.filter((item) => group(item) === 2).map((item) => item.id).sort(), ["around-the-clock", "cursed"]);
  assert.ok(list.findIndex((item) => item.id === "almost") < list.findIndex((item) => !item.unlocked), "полученное секретное — среди полученных");
  assert.deepEqual(list.slice(-9).map((item) => item.hidden), Array(9).fill(true), "9 неполученных секретных — в самом низу");
});

test("чужой профиль: секретное, которого нет у смотрящего, — замок без названия, описания и редкости", async (t) => {
  const other = "76561198000000222";
  const { client } = fixture(t, { request: async (url) => url.endsWith("/v2/runner/profile")
    ? reply({ profile: card({ steamId64: other, details: { secrets: ["piano", "good-memory", "wake-up", "scout"] } }) })
    : reply({ profile: card() }) });
  client.token = token;
  client.profile = require("../src/main/online-profile/profile-state").normalize(card({ details: { secrets: ["wake-up"] } }));
  client.secrets = ["hesoyam"];
  const view = await client.view(other);
  const item = (id) => view.achievements.find((entry) => entry.id === id);
  const sealed = view.achievements.filter((entry) => entry.unlocked && entry.hidden);
  assert.equal(sealed.length, 2);
  for (const entry of sealed) {
    assert.deepEqual([entry.title, entry.description, entry.tier], ["#L_ACH_SECRET_TITLE", "#L_ACH_SEALED_DESC", null]);
    assert.match(entry.id, /^secret-\d+$/, "вместо id — нейтральная метка");
  }
  assert.equal(item("wake-up").title, "#L_ACH_WAKE_TITLE", "открытое у смотрящего видно целиком");
  assert.equal(item("hesoyam").hidden, true, "у того игрока его нет — просто замок");
  assert.equal(item("hesoyam").unlocked, false);
  assert.equal(view.summary.unlocked, 5, "first-launch, scout, wake-up и два скрытых");
  // Окно не получает ни список полученного, ни id секретных, которых смотрящий не открыл.
  assert.equal("secrets" in view.details, false);
  const json = JSON.stringify(view);
  for (const id of ["piano", "good-memory", "back-to-past", "almost", "night-shift", "found-self", "meet-the-maker", "impossible"])
    assert.ok(!json.includes(`"${id}"`), id);
  assert.ok(!json.includes("#L_ACH_PIANO_TITLE"));
  assert.equal(new Set(view.achievements.map((entry) => entry.id)).size, view.achievements.length, "ключи списка разные");
  const own = client.snapshot().achievements;
  assert.equal(own.find((entry) => entry.id === "hesoyam").title, "#L_ACH_HESOYAM_TITLE", "своё открытое — с названием");
  assert.equal(own.some((entry) => entry.id === "piano"), false, "своё неполученное — замок без id");
  assert.equal(own.filter((entry) => entry.hidden).every((entry) => entry.description === "#L_ACH_SECRET_DESC" && /^secret-\d+$/.test(entry.id)), true);
});

test("100 %: все достижения, включая секретные и серверные, кроме бонусного «Царя горы»", () => {
  const achievements = require("../src/main/profile/achievements");
  const profileState = require("../src/main/online-profile/profile-state");
  const all = achievements.ACHIEVEMENTS.filter((item) => ["event", "secret", "server"].includes(item.kind)).map((item) => item.id);
  const games = { "hl1-won": { totalPlaySeconds: 1000 * 3600, launchCount: 1000 } };
  const summary = (secrets) => profileState.state(profileState.normalize(card({ games, details: { secrets } })), {}, true).summary;
  const full = summary(all);
  assert.deepEqual([full.unlocked, full.total, full.percent], [37, 37, 100], "с «Царём горы» не 38 из 37");
  const noKing = summary(all.filter((id) => id !== "king-of-the-hill"));
  assert.deepEqual([noKing.unlocked, noKing.total, noKing.percent], [37, 37, 100], "100 % без «Царя горы»");
  const almost = summary(all.filter((id) => id !== "hesoyam"));
  assert.deepEqual([almost.unlocked, almost.total], [36, 37]);
  assert.equal(profileState.state(null).summary.total, 37);
});

test("«Царь горы»: секретный — замок внизу, у получившего радуга сверху, у чужого маска как у секретных", async (t) => {
  const achievements = require("../src/main/profile/achievements");
  const profileState = require("../src/main/online-profile/profile-state");
  assert.ok(achievements.SECRET_IDS.includes("king-of-the-hill"));
  assert.ok(achievements.SERVER_IDS.includes("king-of-the-hill"), "выдаёт по-прежнему сервер");
  const own = (secrets) => profileState.state(profileState.normalize(card({ details: { secrets } })), {}, true);
  const closed = own([]);
  const hidden = closed.achievements.filter((item) => item.hidden);
  assert.equal(hidden.length, 11, "10 секретных и «Царь горы» — под замком");
  assert.deepEqual(closed.achievements.slice(-11).map((item) => item.hidden), Array(11).fill(true), "в самом низу");
  assert.ok(hidden.every((item) => item.tier === null && /^secret-\d+$/.test(item.id)));
  assert.ok(!JSON.stringify(closed).includes("king-of-the-hill") && !JSON.stringify(closed).includes("#L_ACH_KING_TITLE"));
  assert.deepEqual([closed.summary.unlocked, closed.summary.total], [1, 37]);
  const open = own(["king-of-the-hill"]);
  assert.deepEqual([open.achievements[0].id, open.achievements[0].tier, open.achievements[0].title, open.achievements[0].unlocked],
    ["king-of-the-hill", "rainbow", "#L_ACH_KING_TITLE", true], "радужная карточка сверху");
  assert.deepEqual([open.summary.unlocked, open.summary.total], [1, 37], "полученный не прибавляется к X");

  const other = "76561198000000222";
  const { client } = fixture(t, { request: async (url) => url.endsWith("/v2/runner/profile")
    ? reply({ profile: card({ steamId64: other, details: { secrets: ["king-of-the-hill", "scout"] } }) })
    : reply({ profile: card() }) });
  client.token = token;
  client.profile = profileState.normalize(card());
  const foreign = await client.view(other);
  const sealed = foreign.achievements.filter((item) => item.unlocked && item.hidden);
  assert.equal(sealed.length, 1);
  assert.deepEqual([sealed[0].title, sealed[0].description, sealed[0].tier], ["#L_ACH_SECRET_TITLE", "#L_ACH_SEALED_DESC", null]);
  const json = JSON.stringify(foreign);
  assert.ok(!json.includes("king-of-the-hill") && !json.includes("#L_ACH_KING_TITLE"), "у смотрящего его нет — ни id, ни названия");
  assert.deepEqual([foreign.summary.unlocked, foreign.summary.total], [2, 37], "first-launch и scout");
  await new Promise(setImmediate); // ответ на «Разведку» перезаписывает свой профиль
  client.profile =profileState.normalize(card({ details: { secrets: ["king-of-the-hill"] } }));
  assert.equal((await client.view(other)).achievements[0].id, "king-of-the-hill", "у смотрящего тоже есть — видно целиком");
});

test("сводка профиля: запуски игр у своего и чужого, серия входов только у своего", async (t) => {
  const other = "76561198000000222";
  const games = { "hl1-won": { launchCount: 7, totalPlaySeconds: 60 }, "hl2-2707": { launchCount: 5 } };
  const { client } = fixture(t, { now: () => new Date(2026, 10, 8, 12).getTime(), request: async (url) => url.endsWith("/v2/runner/profile")
    ? reply({ profile: card({ steamId64: other, games }) }) : reply({ profile: card() }) });
  client.token = token;
  client.profile = require("../src/main/online-profile/profile-state").normalize(card({ games }));
  assert.equal(client.snapshot().stats.launches, 12);
  assert.equal(client.snapshot().streak, null, "дней ещё не было");
  client.noteActiveDay();
  assert.deepEqual(client.snapshot().streak, { days: 1, best: 1 });
  const view = await client.view(other);
  assert.equal(view.stats.launches, 12);
  assert.equal("streak" in view, false, "серию чужого сервер не знает");
});

test("серии 7/14/30/100 дней: пропуск начинает заново, рекорд хранится, полученное остаётся", async (t) => {
  const streak = require("../src/main/profile/launch-streak");
  let clock = 0;
  const { root, client } = fixture(t, { now: () => clock, request: async () => reply({ profile: card() }) });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  const day = (n) => new Date(2026, 0, n, 12).getTime();
  const goals = { 7: "week-streak", 14: "hard-worker", 30: "day-after-day", 100: "keep-the-flame" };
  for (let n = 1; n <= 100; n += 1) {
    clock = day(n);
    client.noteActiveDay();
    for (const [goal, id] of Object.entries(goals)) assert.equal(client.secrets.includes(id), n >= Number(goal), `${id} на ${n}-й день`);
  }
  clock = day(102);
  client.noteActiveDay();
  assert.deepEqual(streak.stats(root, clock), { days: 1, best: 100 });
  assert.deepEqual(client.secrets.slice().sort(), Object.values(goals).sort(), "после пропуска полученное на месте");
  assert.deepEqual(streak.stats(root, day(105)), { days: 0, best: 100 }, "давно не заходил — серии нет, рекорд есть");

  const fresh = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-streak-"));
  t.after(() => fs.rmSync(fresh, { recursive: true, force: true }));
  for (let n = 1; n <= 13; n += 1) streak.record(fresh, day(n));
  streak.record(fresh, day(15));
  for (let n = 16; n <= 28; n += 1) streak.record(fresh, day(n));
  assert.deepEqual(streak.stats(fresh, day(28)), { days: 14, best: 14 }, "13 дней, пропуск, 14 дней — «Работяга» только со второй серии");
  // Файл без рекорда (до 0.4.9): рекордом становится текущая серия.
  fs.writeFileSync(path.join(fresh, streak.FILE_NAME), JSON.stringify({ last: "2026-01-28", streak: 9 }));
  assert.deepEqual(streak.stats(fresh, day(28)), { days: 9, best: 9 });
  await new Promise(setImmediate);
});

test("серия: часы назад и перелёт на запад не обнуляют серию", (t) => {
  const streak = require("../src/main/profile/launch-streak");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-streak-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const day = (n, hour) => new Date(2026, 0, n, hour === undefined ? 12 : hour).getTime();
  for (let n = 1; n <= 20; n += 1) streak.record(root, day(n));
  assert.equal(streak.record(root, day(19)), 20, "часы на день назад — та же серия");
  assert.deepEqual(streak.stats(root, day(19)), { days: 20, best: 20 });
  assert.equal(streak.record(root, day(21)), 21, "часы вернули — серия идёт дальше");
  // Перелёт на запад: в 01:00 дня 22 отметились, по новому поясу снова вечер дня 21.
  assert.equal(streak.record(root, day(22, 1)), 22);
  assert.equal(streak.record(root, day(21, 18)), 22);
  assert.equal(streak.record(root, day(22, 12)), 22);
  assert.equal(streak.record(root, day(23)), 23);
});

test("день серии засчитывается по фокусу окна без перезапуска, в тот же день — без лишних запросов", async (t) => {
  const streak = require("../src/main/profile/launch-streak");
  let clock = new Date(2026, 0, 7, 12).getTime();
  const calls = [];
  const { root, client } = fixture(t, { now: () => clock, request: async (url) => { calls.push(url); return reply({ profile: card() }); } });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  fs.writeFileSync(path.join(root, streak.FILE_NAME), JSON.stringify({ last: streak.dayOf(new Date(2026, 0, 6, 12).getTime()), streak: 6 }));
  client.noteActiveDay();
  assert.ok(client.secrets.includes("week-streak"));
  await new Promise(setImmediate);
  const sent = calls.length;
  clock += 3 * 3600 * 1000;
  client.noteNewDay();
  await new Promise(setImmediate);
  assert.equal(calls.length, sent, "тот же день: ни записи, ни повторной выдачи");
  clock = new Date(2026, 0, 8, 0, 30).getTime();
  client.noteNewDay();
  assert.deepEqual(streak.stats(root, clock), { days: 8, best: 8 }, "лаунчер открыт со вчера — новый день засчитан");
  await new Promise(setImmediate);
  const app = fs.readFileSync(path.join(__dirname, "../src/main/AppManager.js"), "utf8");
  assert.match(app, /this\.onlineProfiles\.noteNewDay\(\)/);
});

test("24/7 после перезапуска лаунчера: время запуска — от проверенного старта процесса", () => {
  const EventEmitter = require("node:events");
  const GameDelivery = require("../src/main/game-delivery");
  const processService = new EventEmitter();
  const got = [];
  let seconds = 3600;
  const context = Object.assign(Object.create(GameDelivery.prototype), {
    service: { processService, installService: { readInstalled: () => null }, checkForUpdatesIfDue() {} },
    dumpSnapshots: new Map(), processStarts: new Map(), onlineProfiles: null, tray: null, steamAccount: null, runLogReader: null,
    ensureStatsStore: () => ({ sessionStart() {}, sessionEnd: () => ({ seconds }) }),
    telemetryGameRunning() {}, telemetryGameFinished() {}, emitTo() {}, emitPresence() {}, startTimerWithGame() {}, handleCrash() {},
    achieve: (id) => got.push(id) });
  context.attachProcessEvents();
  const exit = { gameId: "hl1-won", lastExit: { outcome: "exited", requested: false, exitCode: 0 } };
  const ago = (hours) => new Date(Date.now() - hours * 3600 * 1000).toISOString();
  // Подхваченный процесс: статистика начала сессию заново час назад, а игра идёт 25 часов.
  processService.emit("running", { gameId: "hl1-won", startedAt: ago(25) });
  processService.emit("finished", exit);
  assert.deepEqual(got, ["marathon", "around-the-clock"]);
  got.length = 0;
  processService.emit("running", { gameId: "hl1-won", startedAt: ago(1) });
  processService.emit("finished", exit);
  assert.deepEqual(got, []);
  seconds = 4 * 3600;
  processService.emit("running", { gameId: "hl1-won", startedAt: "битая дата" });
  processService.emit("finished", exit);
  assert.deepEqual(got, ["marathon"], "без времени старта — по сессии, как раньше");
});

test("24/7: сутки за один запуск, «Марафон» — от 3 часов", () => {
  const achievements = require("../src/main/profile/achievements");
  assert.deepEqual(achievements.sessionAchievements(3 * 3600 - 1), []);
  assert.deepEqual(achievements.sessionAchievements(3 * 3600), ["marathon"]);
  assert.deepEqual(achievements.sessionAchievements(24 * 3600 - 1), ["marathon"]);
  assert.deepEqual(achievements.sessionAchievements(24 * 3600), ["marathon", "around-the-clock"]);
});

test("«Встреча с создателем»: в поиске есть профиль со значком разработчика", async (t) => {
  let found = [card({ steamId64: "76561198000000222", badges: ["verified"] })];
  const { client } = fixture(t, { request: async (url) => url.endsWith("/v2/runner/search") ? reply({ profiles: found }) : reply({ profile: card() }) });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  await client.search("runner");
  assert.equal(client.secrets.includes("meet-the-maker"), false);
  found = [card({ steamId64: "76561198000000333", badges: ["developer", "verified"] })];
  await client.search("dev");
  assert.ok(client.secrets.includes("meet-the-maker"));
});

test("«Невозможно»: в профиле сохранена страна KP", async (t) => {
  let country = "RU";
  const { client } = fixture(t, { request: async () => reply({ profile: card({ details: { country } }) }) });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  await client.action("customize", { country });
  assert.equal(client.secrets.includes("impossible"), false);
  country = "KP";
  await client.action("customize", { country });
  assert.ok(client.secrets.includes("impossible"));
});

test("интерфейс открывает только свои достижения, поздравление — один раз, список переживает перезапуск", async (t) => {
  const unlocked = [];
  const { root, client } = fixture(t, { onUnlock: (item) => unlocked.push(item), request: async () => reply({ profile: card() }) });
  await assert.rejects(client.unlockFromRenderer("full-set"));
  await assert.rejects(client.unlock("made-up"));
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  assert.equal((await client.unlockFromRenderer("wake-up")).unlocked, true);
  assert.equal((await client.unlockFromRenderer("wake-up")).unlocked, false);
  client.achieve("marathon");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(unlocked.map((item) => item.id), ["wake-up", "marathon"]);
  assert.equal(unlocked[0].title, "#L_ACH_WAKE_TITLE");
  const restarted = new Service({ userDataDir: root, accountProvider: () => ({}) });
  t.after(() => restarted.stop());
  assert.deepEqual(restarted.secrets, ["wake-up", "marathon"]);
});

test("без входа достижения не открываются и не копятся до входа", async (t) => {
  const unlocked = [];
  const { root, client } = fixture(t, { onUnlock: (item) => unlocked.push(item) });
  assert.equal((await client.unlockFromRenderer("wake-up")).unlocked, false);
  client.achieve("marathon");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(unlocked, []);
  assert.deepEqual(client.secrets, []);
  assert.equal(fs.existsSync(path.join(root, "secrets.json")), false);
});

test("свой профиль в поиске открывает «Нашёл себя», чужой профиль — «Разведку»", async (t) => {
  const other = "76561198000000222";
  const { client } = fixture(t, { request: async (url) => url.endsWith("/v2/runner/search")
    ? reply({ profiles: [card({ steamId64: other }), card()] })
    : url.endsWith("/v2/runner/profile") ? reply({ profile: card({ steamId64: other }) }) : reply({ profile: card() }) });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  await client.view(other);
  await client.search("Fake");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(client.secrets.slice().sort(), ["found-self", "scout"]);
});

test("без входа чужие профили и поиск не открываются", async (t) => {
  let calls = 0;
  const { client } = fixture(t, { request: async () => { calls++; return reply({ profile: card() }); } });
  await assert.rejects(client.view("76561198000000222"), /RUNNER_AUTH_REQUIRED/);
  await assert.rejects(client.search("Fake"), /RUNNER_AUTH_REQUIRED/);
  assert.equal(calls, 0);
});

test("достижение, которого сервер не знает, не мешает дослать остальные", async (t) => {
  const sent = [];
  const { client } = fixture(t, { request: async (url, options) => {
    if (!url.endsWith("/v2/runner/secret")) return reply({ profile: card() });
    const id = JSON.parse(options.body).id;
    sent.push(id);
    return id === "chronicler" ? reply({ error: "Unknown secret" }, 400) : reply({ profile: card({ details: { secrets: sent.filter((x) => x !== "chronicler") } }) });
  } });
  client.secrets = ["chronicler", "scout"];
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  await client.sync();
  assert.deepEqual(sent, ["chronicler", "scout"]);
  assert.deepEqual(client.profile.details.secrets, ["scout"]);
});

const atOf = (client, achievementId) => client.snapshot().achievements.find((item) => item.id === achievementId).at;
const readTimes = (root, account = id) => JSON.parse(fs.readFileSync(path.join(root, "achievement-times.json"), "utf8"))[account];

test("время получения: новое достижение — со временем, повторный unlock его не меняет", async (t) => {
  let clock = Date.UTC(2026, 10, 8, 2, 41);
  const { root, client } = fixture(t, { now: () => clock, request: async () => reply({ profile: card() }) });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  await client.unlock("marathon");
  assert.equal(atOf(client, "marathon"), "2026-11-08T02:41:00.000Z");
  clock += 3600 * 1000;
  await client.unlock("marathon");
  assert.equal(atOf(client, "marathon"), "2026-11-08T02:41:00.000Z");
  assert.deepEqual(readTimes(root), { marathon: Date.UTC(2026, 10, 8, 2, 41) });
});

test("время получения: полученное до первой записи файла и пришедшее с другого ПК — без даты", async (t) => {
  const clock = Date.UTC(2026, 10, 8, 2, 41);
  const { root, client } = fixture(t, { now: () => clock, request: async () => reply({ profile: card() }) });
  client.secrets = ["scout"];
  client.token = token;
  client.profile = require("../src/main/online-profile/profile-state").normalize(card({ games: { "hl1-won": { launchCount: 40 } }, details: { secrets: ["marathon"] } }));
  await client.unlock("wake-up");
  assert.deepEqual(readTimes(root), { warmup: null, marathon: null, scout: null, "wake-up": clock });
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 40 } }, details: { secrets: ["marathon", "own-binds", "wake-up"] } }) });
  assert.equal(readTimes(root)["own-binds"], null);
  for (const achievementId of ["warmup", "marathon", "scout", "own-binds"]) assert.equal(atOf(client, achievementId), null, achievementId);
  assert.equal(atOf(client, "wake-up"), "2026-11-08T02:41:00.000Z");
});

test("время получения: порог запусков и часов — когда лаунчер впервые его увидел, first-launch — дата регистрации", (t) => {
  let clock = Date.UTC(2026, 10, 1);
  const { client } = fixture(t, { now: () => clock });
  client.token = token;
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 24 } } }) });
  assert.equal(atOf(client, "warmup"), null);
  clock = Date.UTC(2026, 10, 8, 2, 41);
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 25 } } }) });
  assert.equal(atOf(client, "warmup"), "2026-11-08T02:41:00.000Z");
  clock = Date.UTC(2026, 10, 9);
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 30, totalPlaySeconds: 50 * 3600 } } }) });
  assert.equal(atOf(client, "warmup"), "2026-11-08T02:41:00.000Z");
  assert.equal(atOf(client, "touch-grass"), "2026-11-09T00:00:00.000Z");
  assert.equal(atOf(client, "first-launch"), "2026-10-05T10:00:00Z");
  const achievements = require("../src/main/profile/achievements");
  assert.equal(achievements.evaluate({ firstRunAt: "2026-10-05T10:00:00Z", times: { "first-launch": 1 } })[0].at, "2026-10-05T10:00:00Z");
});

test("время получения: испорченный файл времени не ломает список, плоский файл первой сборки — у вошедшего аккаунта", (t) => {
  const clock = Date.UTC(2026, 10, 8, 2, 41);
  const { root, client: first } = fixture(t);
  first.token = token; first.profile = require("../src/main/online-profile/profile-state").normalize(card());
  first.save();
  fs.writeFileSync(path.join(root, "secrets.json"), JSON.stringify(["marathon", "scout"]));
  const open = (times) => {
    fs.writeFileSync(path.join(root, "achievement-times.json"), times);
    const client = new Service({ userDataDir: root, accountProvider: () => ({}), now: () => clock });
    t.after(() => client.stop());
    return client;
  };
  const broken = open("{битый");
  assert.equal(broken.snapshot().achievements.find((item) => item.id === "marathon").unlocked, true);
  assert.equal(atOf(broken, "marathon"), null);
  broken.accept({ profile: card() });
  assert.deepEqual(readTimes(root), { marathon: null, scout: null });
  const junk = open(JSON.stringify({ marathon: "вчера", scout: -1, "made-up": 5, "first-launch": 7, "wake-up": 1e300 }));
  assert.deepEqual([atOf(junk, "marathon"), atOf(junk, "scout"), atOf(junk, "first-launch")], [null, null, "2026-10-05T10:00:00Z"]);
  assert.equal(atOf(open(JSON.stringify({ [id]: { marathon: clock } })), "marathon"), "2026-11-08T02:41:00.000Z");
  assert.equal(atOf(open(JSON.stringify({ "76561198000000222": { marathon: clock } })), "marathon"), null);
  const flat = open(JSON.stringify({ marathon: clock }));
  assert.equal(atOf(flat, "marathon"), "2026-11-08T02:41:00.000Z");
  flat.accept({ profile: card() });
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(root, "achievement-times.json"), "utf8")), { [id]: { marathon: clock, scout: null } });
});

test("время получения: у каждого аккаунта своё — второй аккаунт с пройденными порогами без дат, у первого даты на месте", (t) => {
  const other = "76561198000000222";
  let clock = Date.UTC(2026, 10, 1);
  const { root, client } = fixture(t, { now: () => clock });
  client.token = token;
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 24 } } }) });
  clock = Date.UTC(2026, 10, 8, 2, 41);
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 25 } } }) });
  assert.equal(atOf(client, "warmup"), "2026-11-08T02:41:00.000Z");
  client.clearAuth(); client.token = token;
  clock = Date.UTC(2026, 10, 9);
  client.accept({ profile: card({ steamId64: other, games: { "hl1-won": { launchCount: 40, totalPlaySeconds: 60 * 3600 } }, details: { secrets: ["scout"] } }) });
  for (const achievementId of ["warmup", "touch-grass", "scout"]) assert.equal(atOf(client, achievementId), null, achievementId);
  assert.deepEqual(readTimes(root, other), { warmup: null, "touch-grass": null, scout: null });
  client.clearAuth(); client.token = token;
  client.accept({ profile: card({ games: { "hl1-won": { launchCount: 30 } } }) });
  assert.equal(atOf(client, "warmup"), "2026-11-08T02:41:00.000Z");
  assert.deepEqual(readTimes(root), { warmup: Date.UTC(2026, 10, 8, 2, 41) });
});

test("окно профиля: дата получения — только у своего профиля и только известная", () => {
  const vue = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/ProfileModal.vue"), "utf8");
  assert.match(vue, /v-if="!targetSteamId && item\.at"/);
});

test("серия дней для «Недели в Мезе»: тот же день не считается, пропуск начинает заново", (t) => {
  const streak = require("../src/main/profile/launch-streak");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-streak-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const day = (n, hour) => new Date(2026, 9, n, hour || 12).getTime();
  assert.equal(streak.record(root, day(1)), 1);
  assert.equal(streak.record(root, day(1, 23)), 1);
  for (let n = 2; n <= 7; n += 1) assert.equal(streak.record(root, day(n, n % 2 ? 0 : 23)), n);
  assert.equal(streak.record(root, day(9)), 1);
  assert.equal(streak.previousDay("2026-03-01"), "2026-02-28");
});

test("модерация: о каждой записи одно уведомление, бан и запрет статуса видны в состоянии", async (t) => {
  const notices = [];
  let moderation = { ban: { id: 7, reason: "Читы", createdAt: "2026-10-09T10:00:00Z", expiresAt: "2026-10-16T10:00:00Z" }, mute: null,
    notices: [{ id: 7, action: "ban", reason: "Читы", createdAt: "2026-10-09T10:00:00Z", expiresAt: "2026-10-16T10:00:00Z" },
      { id: 6, action: "reset-nick", reason: "", createdAt: "2026-10-09T09:00:00Z", expiresAt: null },
      { id: 5, action: "hack", reason: "x", createdAt: "2026-10-09T09:00:00Z", expiresAt: null }] };
  const request = async () => reply({ profile: card(), moderation });
  const { root, client } = fixture(t, { request, onModeration: (item) => notices.push(item), now: () => Date.parse("2026-10-10T00:00:00Z") });
  client.token = token; client.profile = card();
  await client.sync();
  assert.deepEqual(notices.map((item) => item.action), ["reset-nick", "ban"], "неизвестное действие пропущено, старые сверху");
  assert.equal(client.snapshot().ban.reason, "Читы");
  await client.sync();
  assert.equal(notices.length, 2, "повторная синхронизация повторила уведомление");
  const restarted = new Service({ userDataDir: root, request, onModeration: (item) => notices.push(item) });
  t.after(() => restarted.stop());
  restarted.token = token; restarted.profile = card();
  await restarted.sync();
  assert.equal(notices.length, 2, "после перезапуска уведомление пришло снова");

  // Срок бана вышел — состояние его больше не показывает и без синхронизации.
  const later = new Service({ userDataDir: root, request, now: () => Date.parse("2026-10-17T00:00:00Z") });
  t.after(() => later.stop());
  later.token = token; later.profile = card();
  await later.sync();
  assert.equal(later.snapshot().ban, null);

  // Запрет статуса приходит отказом на сохранение: состояние его подхватывает.
  moderation = { ban: null, mute: null, notices: [] };
  const muted = new Service({ userDataDir: root, request: async (url) => url.endsWith("/customize")
    ? reply({ error: "MUTED", mute: { id: 9, reason: "Спам", createdAt: "2026-10-09T10:00:00Z", expiresAt: null } }, 403)
    : reply({ profile: card(), moderation }) });
  t.after(() => muted.stop());
  muted.token = token; muted.profile = card();
  await assert.rejects(muted.call("/v2/runner/customize", { status: "x" }), { code: "RUNNER_MUTED" });
  assert.equal(muted.snapshot().mute.reason, "Спам");
});

test("avatar frame and custom badges from the server pass the same checks as on the server", () => {
  const schema = require("../src/main/online-profile/profile-state");
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="#f0f"/></svg>';
  const profile = schema.normalize({
    steamId64: "76561198000000001",
    badges: ["developer", "c-0a1b2c3d", "c-ffffffff", "c-0000aaaa", "unknown"],
    customBadges: [
      { id: "c-0a1b2c3d", label: { ru: "Чемпион", en: "" }, note: { ru: "За победу" }, svg },
      { id: "c-0000aaaa", label: { ru: "В цвет темы" }, svg, themed: true },
      { id: "c-12345678", label: { ru: "" }, svg },
      { id: "c-87654321", label: { ru: "Не SVG" }, svg: "<html></html>" },
      { id: "bad", label: { ru: "Неверный id" }, svg },
    ],
    avatarFrame: { css: "background: conic-gradient(from var(--frame-angle), #f0f, #0ff); animation: frame-spin 3s linear infinite" },
  });
  assert.deepEqual(profile.badges, ["developer", "c-0a1b2c3d", "c-0000aaaa"], "значок без описания или неизвестный не отсеян");
  assert.equal(profile.customBadges.length, 2);
  assert.deepEqual(profile.customBadges.map((badge) => badge.themed), [false, true], "потерян флаг стилизации");
  assert.equal(profile.customBadges[0].label.en, "Чемпион", "нет запасной подписи на английском");
  assert.ok(profile.customBadges[0].icon.startsWith("data:image/svg+xml;base64,"), "иконка не data:-адрес");
  assert.match(profile.avatarFrame.css, /frame-spin/);
  const state = schema.state(profile, {}, true);
  assert.deepEqual(state.avatarFrame, profile.avatarFrame);
  assert.equal(state.customBadges.length, 2);

  for (const css of ["background: url(https://evil.example/x.png)", "width: 300px", "inset: -40px", "transform: scale(4)"]) {
    assert.equal(schema.normalize({ steamId64: "76561198000000001", avatarFrame: { css } }).avatarFrame, null, "принят CSS рамки: " + css);
  }
});

test("онлайн и текущая игра: чужой профиль берёт их с сервера, свой — из лаунчера", () => {
  const schema = require("../src/main/online-profile/profile-state");
  const id = "76561198000000001";

  const playing = schema.state(schema.normalize({ steamId64: id, presence: { online: true, gameId: "hl1-won" } }), {}, true);
  assert.deepEqual(playing.presence, { online: true, gameId: "hl1-won", launcherId: "70" });

  // Игра без отметки онлайна — всё равно в сети: пульс игры идёт через сервер
  assert.equal(schema.state(schema.normalize({ steamId64: id, presence: { gameId: "ghosting-renewed" } }), {}, true).presence.online, true);
  // Старый сервер без presence и неизвестная игра
  assert.deepEqual(schema.state(schema.normalize({ steamId64: id }), {}, true).presence, { online: false, gameId: null, launcherId: null });
  assert.equal(schema.normalize({ steamId64: id, presence: { online: true, gameId: "cs16" } }).presence.gameId, null);

  // Свой профиль без связи серый, даже если игра запущена
  const own = schema.state(schema.normalize({ steamId64: id, presence: { online: true } }), {}, false, [], { online: false, gameId: "hl2-4260" });
  assert.deepEqual(own.presence, { online: false, gameId: "hl2-4260", launcherId: "220" });
});

test("рейтинг: голос уходит на сервер, пауза и лимит лайков возвращают текущий голос, за себя нельзя", async (t) => {
  const other = "76561198000000222";
  const bodies = [];
  let cooldown = false;
  let likeLimit = false;
  const { client } = fixture(t, { request: async (url, options) => {
    if (url.endsWith("/v2/runner/profile")) return reply({ profile: card({ steamId64: other, rating: { score: -12, likes: 1, dislikes: 13, dubious: true } }), vote: { value: -1, nextAt: null } });
    if (!url.endsWith("/v2/runner/rate")) return reply({ profile: card() });
    bodies.push(JSON.parse(options.body));
    if (likeLimit) return reply({ error: "LIKE_LIMIT", likesAt: "2026-10-12T16:30:00Z", rating: { score: 0, likes: 0, dislikes: 0, dubious: false }, vote: { value: 0, nextAt: null } }, 429);
    if (cooldown) return reply({ error: "RATE_COOLDOWN", rating: { score: 1, likes: 1, dislikes: 0, dubious: false }, vote: { value: 1, nextAt: "2026-10-12T15:00:00Z" } }, 429);
    return reply({ rating: { score: 1, likes: 1, dislikes: 0, dubious: false }, vote: { value: 1, nextAt: "2026-10-12T15:00:00Z" } });
  } });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  const view = await client.view(other);
  assert.deepEqual(view.rating, { score: -12, likes: 1, dislikes: 13, dubious: true });
  assert.equal(view.vote.value, -1);
  const voted = await client.rate(other, 1);
  assert.equal(voted.status, "ok");
  assert.deepEqual(bodies[0], { steamId64: other, value: 1 });
  cooldown = true;
  const waiting = await client.rate(other, -1);
  assert.equal(waiting.status, "cooldown");
  assert.equal(waiting.vote.value, 1);
  // Шестой лайк за 4 часа: сервер говорит, когда освободится следующий
  likeLimit = true;
  const limited = await client.rate("76561198000000333", 1);
  assert.equal(limited.status, "like-limit");
  assert.equal(limited.likesAt, "2026-10-12T16:30:00Z");
  assert.equal(limited.vote.value, 0);
  await assert.rejects(client.rate(id, 1), /RUNNER_SELF_VOTE/);
  await assert.rejects(client.rate(other, 2), /RUNNER_INVALID_ID/);
});

test("игроки: без входа сервер не спрашивается, запрос проверяется, ответ разбирается как карточки", async (t) => {
  const other = "76561198000000222";
  const sent = [];
  const { client } = fixture(t, { request: async (url, options) => {
    sent.push(JSON.parse(options.body));
    return url.endsWith("/v2/runner/players") && JSON.parse(options.body).view === "online"
      ? reply({ online: { total: 3, playing: 1, profiles: [card({ steamId64: other, presence: { online: true, gameId: "hl1-won" } }), { steamId64: "bad" }] } })
      : reply({ top: { more: true, game: "hl1-won", profiles: [card({ rank: 51, achievementCount: 4, details: { secrets: ["scout"] } })] } });
  } });
  assert.deepEqual(await client.players({ view: "online" }), { status: "ok", signedIn: false });
  assert.equal(sent.length, 0);
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  const online = await client.players({ view: "online" });
  assert.equal(online.total, 3);
  assert.equal(online.playing, 1);
  assert.deepEqual(online.profiles.map((profile) => profile.steamId64), [other]);
  assert.deepEqual(online.profiles[0].presence, { online: true, gameId: "hl1-won", launcherId: "70" });
  const top = await client.players({ view: "top", sort: "hours", game: "hl1-won", offset: 50 });
  assert.deepEqual(sent[sent.length - 1], { view: "top", sort: "hours", game: "hl1-won", offset: 50 });
  assert.equal(top.more, true);
  assert.equal(top.game, "hl1-won");
  assert.equal(top.profiles[0].rank, 51);
  assert.equal(top.profiles[0].achievementCount, 4);
  const count = sent.length;
  for (const query of [{ sort: "nope" }, { sort: "hours", game: "hl3" }, { sort: "hours", offset: 7 }, { sort: "hours", offset: 500 }, { sort: "hours", offset: "0" }])
    await assert.rejects(client.players(Object.assign({ view: "top" }, query)), /RUNNER_INVALID_QUERY/);
  assert.equal(sent.length, count);
});

// ── поддержавшие: права, выбранная заготовка подсветки, Telegram ─────────
// Сервер на Go кладёт perks, stylePreset и telegram только в свой профиль и
// только когда есть что сказать; без ключа — права нет и Telegram не
// привязан (так же отвечает сервер на PHP).

test("права, заготовка и Telegram: без ключей — нет права, нет заготовки, не привязан", () => {
  const schema = require("../src/main/online-profile/profile-state");
  const bare = schema.normalize(card());
  assert.deepEqual(bare.perks, { styles: false, subscriber: null });
  assert.equal(bare.stylePreset, null);
  assert.deepEqual(bare.telegram, { linked: false, username: null });

  const full = schema.normalize(card({ perks: { styles: true, subscriber: { active: true, until: "2026-11-30T12:00:00Z" } },
    stylePreset: 3, telegram: { linked: true, username: "hlsr_fan" } }));
  assert.deepEqual(full.perks, { styles: true, subscriber: { active: true, until: "2026-11-30T12:00:00Z" } });
  assert.equal(full.stylePreset, 3);
  assert.deepEqual(full.telegram, { linked: true, username: "hlsr_fan" });

  // Подписка без конца периода (until: null) и подписки не было (subscriber: null).
  assert.deepEqual(schema.normalize(card({ perks: { styles: false, subscriber: { active: false, until: null } } })).perks,
    { styles: false, subscriber: { active: false, until: null } });
  assert.deepEqual(schema.normalize(card({ perks: { styles: true, subscriber: { active: true, until: null } } })).perks,
    { styles: true, subscriber: { active: true, until: null } });
  assert.deepEqual(schema.normalize(card({ perks: { styles: true, subscriber: null } })).perks, { styles: true, subscriber: null });
});

test("права, заготовка и Telegram: мусор с сервера не даёт права и не выдумывает имя", () => {
  const schema = require("../src/main/online-profile/profile-state");
  const perksOf = (perks) => schema.normalize(card({ perks })).perks;
  for (const perks of [null, "yes", 1, true, [], [true], { styles: "true" }, { styles: 1 }])
    assert.deepEqual(perksOf(perks), { styles: false, subscriber: null }, JSON.stringify(perks));
  assert.deepEqual(perksOf({ styles: true, subscriber: { active: "true", until: "не дата" } }), { styles: true, subscriber: { active: false, until: null } });
  for (const subscriber of ["yes", 1, true, [], [{ active: true }]])
    assert.deepEqual(perksOf({ styles: true, subscriber }), { styles: true, subscriber: null }, JSON.stringify(subscriber));

  for (const stylePreset of [0, -1, 1.5, "3", null, true, 2 ** 53, {}, [3]])
    assert.equal(schema.normalize(card({ stylePreset })).stylePreset, null, JSON.stringify(stylePreset));

  const tg = (telegram) => schema.normalize(card({ telegram })).telegram;
  assert.deepEqual(tg({ linked: true, username: null }), { linked: true, username: null });
  assert.deepEqual(tg({ linked: true }), { linked: true, username: null });
  assert.deepEqual(tg({ linked: true, username: "  hlsr_fan " }), { linked: true, username: "hlsr_fan" }, "пробелы по краям срезаются");
  assert.deepEqual(tg({ linked: true, username: "x".repeat(64) }), { linked: true, username: "x".repeat(64) });
  for (const username of ["@hlsr", "hl sr", "хлср", "x".repeat(65), "", "   ", 5, "a<b>", "a.b", ["a"]])
    assert.deepEqual(tg({ linked: true, username }), { linked: true, username: null }, JSON.stringify(username));
  for (const telegram of [null, "yes", true, [], { linked: "true", username: "a" }, { username: "a" }, { linked: false, username: "a" }])
    assert.deepEqual(tg(telegram), { linked: false, username: null }, JSON.stringify(telegram));
});

test("права, заготовка и Telegram: в снимке состояния и после перезапуска (повторный разбор того же вида)", async (t) => {
  const schema = require("../src/main/online-profile/profile-state");
  const raw = card({ perks: { styles: true, subscriber: { active: false, until: null } }, stylePreset: 7, telegram: { linked: true, username: null } });
  const profile = schema.normalize(raw);
  assert.deepEqual(schema.normalize(profile), profile, "разбор уже разобранного профиля (сохранённая сессия) ничего не меняет");

  const state = schema.state(profile, {}, true);
  assert.deepEqual(state.perks, { styles: true, subscriber: { active: false, until: null } });
  assert.equal(state.stylePreset, 7);
  assert.deepEqual(state.telegram, { linked: true, username: null });
  const empty = schema.state(null, {}, false);
  assert.deepEqual(empty.perks, { styles: false, subscriber: null });
  assert.equal(empty.stylePreset, null);
  assert.deepEqual(empty.telegram, { linked: false, username: null });

  const { root, client } = fixture(t, { request: async () => reply({ profile: raw }) });
  client.token = token; client.profile = schema.normalize(card());
  await client.sync();
  assert.equal(client.snapshot().stylePreset, 7);
  assert.deepEqual(client.snapshot().telegram, { linked: true, username: null });
  const restarted = new Service({ userDataDir: root, accountProvider: () => ({ steamId64: id, personaName: "Fake runner" }) });
  t.after(() => restarted.stop());
  assert.deepEqual(restarted.snapshot().perks, { styles: true, subscriber: { active: false, until: null } });
  assert.equal(restarted.snapshot().stylePreset, 7);
  assert.deepEqual(restarted.snapshot().telegram, { linked: true, username: null });
});

test("customize: 409 PERK_REQUIRED и PRESET_UNKNOWN — свои коды ошибок, другие отказы — как раньше", async (t) => {
  const schema = require("../src/main/online-profile/profile-state");
  let answer;
  const bodies = [];
  const { client } = fixture(t, { request: async (url, options) => { bodies.push([url, JSON.parse(options.body)]); return answer; } });
  client.token = token; client.profile = schema.normalize(card());
  const code = async (promise) => { try { await promise; return "ok"; } catch (error) { return error.code; } };

  answer = reply({ error: "PERK_REQUIRED" }, 409);
  assert.equal(await code(client.action("customize", { stylePreset: 3 })), "RUNNER_PERK_REQUIRED");
  assert.deepEqual(bodies[bodies.length - 1], [client.base + "/v2/runner/customize", { stylePreset: 3 }]);
  answer = reply({ error: "PRESET_UNKNOWN" }, 409);
  assert.equal(await code(client.action("customize", { stylePreset: 99 })), "RUNNER_PRESET_UNKNOWN");
  // Один speedrun.com — один профиль HLSR: занят другим — свой код.
  answer = reply({ error: "SPEEDRUN_TAKEN" }, 409);
  assert.equal(await code(client.action("speedrun/link", { apiKey: "FakeKeyForTesting12345" })), "RUNNER_SPEEDRUN_TAKEN");
  answer = reply({ error: "SOMETHING_ELSE" }, 409);
  assert.equal(await code(client.action("customize", { stylePreset: 3 })), "RUNNER_HTTP_409");
  answer = { status: 409, body: "не json", headers: {} };
  assert.equal(await code(client.action("customize", { stylePreset: 3 })), "RUNNER_HTTP_409");
  answer = reply({ error: "PERK_REQUIRED" }, 400);
  assert.equal(await code(client.action("customize", { stylePreset: 3 })), "RUNNER_HTTP_400", "код отказа — только у 409");
  assert.equal(client.snapshot().account.signedIn, true, "отказ не выкидывает из аккаунта");

  // Через IPC интерфейс получает код, а не общий сбой.
  const handlers = new Map();
  require("../src/main/online-profile/ipc").bind({ handle: (name, fn) => handlers.set(name, fn) }, client, () => {});
  answer = reply({ error: "PERK_REQUIRED" }, 409);
  assert.deepEqual(await handlers.get("runner-customize")({}, { stylePreset: 3 }), { status: "error", code: "RUNNER_PERK_REQUIRED" });
  answer = reply({ error: "PRESET_UNKNOWN" }, 409);
  assert.deepEqual(await handlers.get("runner-customize")({}, { stylePreset: 3 }), { status: "error", code: "RUNNER_PRESET_UNKNOWN" });

  // Удачный выбор: сервер отвечает профилем с заготовкой — он сразу в снимке.
  answer = reply({ profile: card({ stylePreset: 3, perks: { styles: true, subscriber: null }, nick: { css: "color: #ffd700" } }) });
  const saved = await handlers.get("runner-customize")({}, { stylePreset: 3 });
  assert.equal(saved.snapshot.stylePreset, 3);
  assert.deepEqual(saved.snapshot.nick, { style: "", css: "color: #ffd700" });
  answer = reply({ profile: card({ perks: { styles: true, subscriber: null } }) });
  assert.equal((await handlers.get("runner-customize")({}, { stylePreset: 0 })).snapshot.stylePreset, null, "0 снимает заготовку");
  assert.deepEqual(bodies[bodies.length - 1][1], { stylePreset: 0 });
});

// ── подписчик: значок у ника и голубая рамка профиля ─────────────────────
// Сервер кладёт "subscriber": true в любую карточку профиля (свой профиль,
// чужой, игроки, поиск), только пока подписка активна; без ключа — нет.

test("подписчик: только true включает значок, без ключа и мусор — false", () => {
  const schema = require("../src/main/online-profile/profile-state");
  assert.equal(schema.normalize(card({ subscriber: true })).subscriber, true);
  assert.equal(schema.normalize(card()).subscriber, false, "без ключа");
  for (const subscriber of ["true", 1, "1", "yes", {}, { active: true }, [true], null, false, 0])
    assert.equal(schema.normalize(card({ subscriber })).subscriber, false, JSON.stringify(subscriber));

  const profile = schema.normalize(card({ subscriber: true }));
  assert.deepEqual(schema.normalize(profile), profile, "сохранённая сессия разбирается так же");
  assert.equal(schema.state(profile, {}, true).subscriber, true);
  assert.equal(schema.state(schema.normalize(card()), {}, true).subscriber, false);
  assert.equal(schema.state(null, {}, false).subscriber, false, "без входа значка нет");
  assert.equal(schema.listEntry(card({ subscriber: true, rank: 3 })).subscriber, true, "строка списка игроков");
  assert.equal(schema.listEntry(card({ subscriber: "true", rank: 3 })).subscriber, false);
});

test("подписчик: флаг доходит до интерфейса — свой профиль, перезапуск, поиск, игроки, чужой профиль", async (t) => {
  const other = "76561198000000222", third = "76561198000000333";
  const { root, client } = fixture(t, { request: async (url, options) => {
    if (url.endsWith("/v2/runner/search")) return reply({ profiles: [card({ steamId64: other, subscriber: true }), card({ steamId64: third, subscriber: 1 })] });
    if (url.endsWith("/v2/runner/players")) return reply({ online: { total: 2, playing: 0, profiles: [card({ steamId64: other, subscriber: true }), card({ steamId64: third })] } });
    if (url.endsWith("/v2/runner/profile")) {
      const wanted = JSON.parse(options.body).steamId64;
      return reply({ profile: card({ steamId64: wanted, subscriber: wanted === other ? true : "true" }) });
    }
    return reply({ profile: card({ subscriber: true }) });
  } });
  client.token = token; client.profile = require("../src/main/online-profile/profile-state").normalize(card());
  assert.equal(client.snapshot().subscriber, false);
  await client.sync();
  assert.equal(client.snapshot().subscriber, true, "свой профиль после синхронизации");
  const restarted = new Service({ userDataDir: root, accountProvider: () => ({ steamId64: id, personaName: "Fake runner" }) });
  t.after(() => restarted.stop());
  assert.equal(restarted.snapshot().subscriber, true, "после перезапуска, до первой синхронизации");

  assert.deepEqual((await client.search("Fake")).map((profile) => [profile.steamId64, profile.subscriber]), [[other, true], [third, false]]);
  assert.deepEqual((await client.players({ view: "online" })).profiles.map((profile) => [profile.steamId64, profile.subscriber]), [[other, true], [third, false]]);
  assert.equal((await client.view(other)).subscriber, true);
  assert.equal((await client.view(third)).subscriber, false);
});

test("0.4.9: новые значки и даты", () => {
  const schema = require("../src/main/online-profile/profile-state");
  const profile = schema.normalize(card({ badges: ["bughunter", "booster", "weird"], boosterSince: 1762400000, supporterSince: 1754000000 }));
  assert.deepEqual(profile.badges, ["bughunter", "booster"]);
  assert.equal(profile.boosterSince, 1762400000);
  assert.equal(profile.supporterSince, 1754000000);
  assert.deepEqual(schema.normalize(profile), profile, "сохранённая сессия разбирается так же");
  for (const bad of ["x", -5, 0, 1.5, null, {}, [1]]) {
    const parsed = schema.normalize(card({ boosterSince: bad, supporterSince: bad }));
    assert.equal(parsed.boosterSince, null, JSON.stringify(bad));
    assert.equal(parsed.supporterSince, null, JSON.stringify(bad));
  }
  const state = schema.state(profile, {}, true);
  assert.equal(state.boosterSince, 1762400000);
  assert.equal(state.supporterSince, 1754000000);
  const empty = schema.state(null, {}, false);
  assert.equal(empty.boosterSince, null);
  assert.equal(empty.supporterSince, null);
});

test("0.4.9: supportMonths — полные календарные месяцы", () => {
  const { supportMonths } = require("../src/shared/support-months");
  const since = Date.UTC(2026, 7, 6) / 1000;
  assert.equal(supportMonths(since, Date.UTC(2026, 10, 6)), 3);
  assert.equal(supportMonths(since, Date.UTC(2026, 10, 5)), 2);
  assert.equal(supportMonths(since, Date.UTC(2026, 7, 20)), 0);
  assert.equal(supportMonths(Date.UTC(2027, 0, 1) / 1000, Date.UTC(2026, 10, 6)), 0, "в будущем");
  assert.equal(supportMonths(0, Date.UTC(2026, 10, 6)), 0);
  assert.equal(supportMonths(null, Date.UTC(2026, 10, 6)), 0);
});

test("discordLink нормализуется: имя до 64 символов, время буста в секундах, мусор — не привязан", () => {
  const schema = require("../src/main/online-profile/profile-state");
  const dl = (discordLink) => schema.normalize(card({ discordLink })).discordLink;
  assert.deepEqual(dl({ linked: true, username: "gordon", boosterSince: 1762400000 }), { linked: true, username: "gordon", boosterSince: 1762400000 });
  assert.deepEqual(dl({ linked: true, username: "gordon" }), { linked: true, username: "gordon", boosterSince: null });
  assert.deepEqual(dl({ linked: true, username: "x".repeat(65) }), { linked: true, username: null, boosterSince: null });
  assert.deepEqual(dl({ linked: true, username: "x".repeat(64), boosterSince: "вчера" }), { linked: true, username: "x".repeat(64), boosterSince: null });
  for (const raw of [undefined, null, "yes", true, [], { linked: "true", username: "a" }, { username: "a" }, { linked: false, username: "a", boosterSince: 1762400000 }])
    assert.deepEqual(dl(raw), { linked: false, username: null, boosterSince: null }, JSON.stringify(raw));
  const profile = schema.normalize(card({ discordLink: { linked: true, username: "gordon", boosterSince: 1762400000 } }));
  assert.deepEqual(schema.normalize(profile).discordLink, profile.discordLink);
  assert.deepEqual(schema.state(profile, {}, true).discordLink, profile.discordLink);
  assert.deepEqual(schema.state(null, {}, false).discordLink, { linked: false, username: null, boosterSince: null });
});

test("discord/code и discord/unlink идут на свои адреса, 429 — RUNNER_HTTP_429", async (t) => {
  const schema = require("../src/main/online-profile/profile-state");
  let answer;
  const urls = [];
  const { client } = fixture(t, { request: async (url) => { urls.push(url); return answer; } });
  client.token = token; client.profile = schema.normalize(card());
  answer = reply({ schemaVersion: 2, code: "123456", expiresAt: 1762400000, profile: card({ discordLink: { linked: false } }) });
  const result = await client.action("discord/code", {});
  assert.equal(result.code, "123456");
  assert.equal(urls[0], client.base + "/v2/runner/discord/code");
  answer = reply({ schemaVersion: 2, profile: card({ discordLink: { linked: false } }) });
  await client.action("discord/unlink", {});
  assert.equal(urls[1], client.base + "/v2/runner/discord/unlink");
  answer = reply({ error: "RATE_LIMITED" }, 429);
  await assert.rejects(client.action("discord/code", {}), { code: "RUNNER_HTTP_429" });
});

test("telegram/code идёт на свой адрес и доступен из IPC (runner-telegram-code), 429 — RUNNER_HTTP_429", async (t) => {
  const schema = require("../src/main/online-profile/profile-state");
  let answer;
  const urls = [];
  const { client } = fixture(t, { request: async (url) => { urls.push(url); return answer; } });
  client.token = token; client.profile = schema.normalize(card());
  const handlers = new Map();
  require("../src/main/online-profile/ipc").bind({ handle: (name, fn) => handlers.set(name, fn) }, client, () => {});
  answer = reply({ schemaVersion: 2, code: "123456", expiresAt: 1762400000, profile: card() });
  const result = await handlers.get("runner-telegram-code")({});
  assert.equal(result.code, "123456");
  assert.equal(result.expiresAt, 1762400000);
  assert.ok(result.snapshot, "снимок профиля для окна");
  assert.deepEqual(urls, [client.base + "/v2/runner/telegram/code"]);
  answer = reply({ error: "RATE_LIMITED" }, 429);
  assert.deepEqual(await handlers.get("runner-telegram-code")({}), { status: "error", code: "RUNNER_HTTP_429" });
});

test("telegram/unlink: IPC runner-telegram-unlink идёт на свой адрес, снимок — уже без привязки", async (t) => {
  const schema = require("../src/main/online-profile/profile-state");
  let answer;
  const urls = [];
  const { client } = fixture(t, { request: async (url) => { urls.push(url); return answer; } });
  client.token = token; client.profile = schema.normalize(card({ telegram: { linked: true, username: "stranger" } }));
  assert.deepEqual(client.snapshot().telegram, { linked: true, username: "stranger" });
  const handlers = new Map();
  require("../src/main/online-profile/ipc").bind({ handle: (name, fn) => handlers.set(name, fn) }, client, () => {});
  answer = reply({ schemaVersion: 2, profile: card() });
  const result = await handlers.get("runner-telegram-unlink")({});
  assert.deepEqual(urls, [client.base + "/v2/runner/telegram/unlink"]);
  assert.deepEqual(result.snapshot.telegram, { linked: false, username: null });
});

test("Спасибо: одна оплата — одно окно, и после перезапуска тоже", async (t) => {
  const thanks = [];
  const request = async () => reply({ profile: card(), supportThanks: [{ id: "t1", kind: "subscription", until: 1800000000, styles: true }] });
  const { root, client } = fixture(t, { request, onSupportThanks: (item) => thanks.push(item) });
  client.token = token; client.profile = card();
  await client.sync();
  await client.sync();
  assert.deepEqual(thanks, [{ id: "t1", kind: "subscription", until: 1800000000, styles: true }]);
  const restarted = new Service({ userDataDir: root, request, onSupportThanks: (item) => thanks.push(item) });
  t.after(() => restarted.stop());
  restarted.token = token; restarted.profile = card();
  await restarted.sync();
  assert.equal(thanks.length, 1, "после перезапуска окно пришло снова");
});

test("Спасибо: мусор отброшен", async (t) => {
  const thanks = [];
  const request = async () => reply({ profile: card(), supportThanks: [{ id: "t1", kind: "trial" }, { id: 5, kind: "donation" }, { id: "x".repeat(200), kind: "donation" }] });
  const { client } = fixture(t, { request, onSupportThanks: (item) => thanks.push(item) });
  client.token = token; client.profile = card();
  await client.sync();
  assert.equal(thanks.length, 0);
});

test("Спасибо: запись в центре уведомлений хранит срок подписки", (t) => {
  const NotificationStore = require("../src/main/notifications/NotificationStore");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-thanks-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const notifications = new NotificationStore({ file: path.join(root, "notifications.json") });
  notifications.add({ level: "info", code: "SUPPORT_THANKS", meta: { until: "2027-01-15T08:00:00Z" } });
  assert.equal(notifications.list().items[0].meta.until, "2027-01-15T08:00:00Z");
});

test("Спасибо: событие до готовности окна ждёт и уходит один раз, после — сразу", () => {
  const { createPendingEvent } = require("../src/main/pending-event");
  const sent = [];
  const event = createPendingEvent((item) => sent.push(item));
  event.push("a");
  assert.deepEqual(sent, [], "ушло в окно, которое ещё не подписалось");
  event.markReady();
  event.markReady();
  assert.deepEqual(sent, ["a"], "после готовности (и перезагрузки окна) событие не ровно одно");
  event.push("b");
  assert.deepEqual(sent, ["a", "b"]);
});

test("Спасибо: две оплаты до готовности окна — уходит новейшая (первая), одна", () => {
  const { createPendingEvent } = require("../src/main/pending-event");
  const sent = [];
  const event = createPendingEvent((item) => sent.push(item));
  event.push("новая");
  event.push("старая");
  event.markReady();
  assert.deepEqual(sent, ["новая"]);
});

test("Спасибо: новые сначала, срок вне диапазона — null", async (t) => {
  const thanks = [];
  const request = async () => reply({ profile: card(), supportThanks: [
    { id: "n", kind: "donation" }, { id: "o", kind: "subscription", until: 253402300800 }, { id: "p", kind: "subscription", until: 253402300799 },
  ] });
  const { client } = fixture(t, { request, onSupportThanks: (item) => thanks.push(item) });
  client.token = token; client.profile = card();
  await client.sync();
  assert.deepEqual(thanks.map((item) => item.id), ["n", "o", "p"], "порядок сервера (новые первыми) сохранён");
  assert.equal(thanks[1].until, null, "срок за 9999 годом дал бы RangeError в toISOString");
  assert.equal(thanks[2].until, 253402300799);
});
