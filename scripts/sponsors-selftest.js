"use strict";

// Как в официальной сборке: эти значения подставляет webpack (src/main/security/official.js).
process.env.HLSR_API_HOST = process.env.HLSR_API_HOST || "api.hlsr.pro";
process.env.HLSR_DISCORD_APP_ID = process.env.HLSR_DISCORD_APP_ID || "100000000000000001";

// Окно «Спонсорство»: клиент /v2/public/sponsors с откатом на /v1 и открытие
// ссылок. Адрес ссылки берётся только из последнего списка клиента, а не из
// аргументов окна.
const assert = require("node:assert/strict");
const test = require("node:test");

const SponsorsClient = require("../src/main/telemetry/sponsors-client");
const sponsorsIpc = require("../src/main/telemetry/sponsors-ipc");

const V2 = "https://api.hlsr.pro/v2/public/sponsors";
const V1 = "https://api.hlsr.pro/v1/public/sponsors";

const STEAM_ID = "76561198000000111";
const AVATAR = "https://avatars.steamstatic.com/aa.jpg";

function reply(body, status = 200) {
  return { status, body: typeof body === "string" ? body : JSON.stringify(body) };
}

// Запросы по адресу: значение — ответ, функция или ошибка сети.
function fakeRequest(routes) {
  const calls = [];
  const request = async (url) => {
    calls.push(url);
    const route = routes[url];
    if (route === undefined) return reply("", 404);
    const value = typeof route === "function" ? route() : route;
    if (value instanceof Error) throw value;
    return value;
  };
  request.calls = calls;
  return request;
}

function listBody(extra) {
  return Object.assign({
    links: {
      subscribe: "https://t.me/hlsr_bot?start=sub",
      donate: "https://t.me/tribute/app?startapp=abc",
      bot: "https://t.me/HLSRSupportBot",
    },
    active: [
      { card: { personaName: "Активный", avatarUrl: AVATAR, nick: { style: "gold" }, avatarFrame: null }, since: "2026-10-30", link: { mode: "profile", steamId64: STEAM_ID } },
    ],
    donors: [
      { card: { personaName: "Крупный", avatarUrl: AVATAR }, name: "Крупный", amountRub: 15000, note: "спасибо", link: { mode: "url", url: "https://example.com/me" } },
      { card: null, name: "Вручную", amountRub: 300, note: "", link: { mode: "none" } },
    ],
    past: [
      { card: { personaName: "Бывший" }, since: "2026-01-02", link: { mode: "none" } },
    ],
  }, extra);
}

function client(routes, options) {
  const request = fakeRequest(routes);
  let now = 1000000;
  const instance = new SponsorsClient(Object.assign({ request, now: () => now }, options));
  return { instance, request, tick: (ms) => { now += ms; } };
}

test("v2 разбирается в три раздела, карточка не требует steamId64", async () => {
  const { instance, request } = client({ [V2]: reply(listBody()) });
  const answer = await instance.get();
  assert.equal(answer.status, "ok");
  assert.equal(answer.version, 2);
  assert.deepEqual(answer.links, {
    subscribe: "https://t.me/hlsr_bot?start=sub",
    donate: "https://t.me/tribute/app?startapp=abc",
    bot: "https://t.me/HLSRSupportBot",
  });
  assert.equal(answer.active.length, 1);
  assert.equal(answer.donors.length, 2);
  assert.equal(answer.past.length, 1);

  const active = answer.active[0];
  assert.deepEqual(Object.keys(active.profile).sort(), ["avatarFrame", "avatarUrl", "nick", "personaName", "subscriber"]);
  assert.equal(active.profile.subscriber, false, "без ключа subscriber значка нет");
  assert.equal(active.profile.personaName, "Активный");
  assert.equal(active.profile.avatarUrl, AVATAR);
  assert.equal(active.name, "");
  assert.equal(active.amount, 0);
  assert.equal(active.note, "");
  assert.equal(active.since, "2026-10-30");
  assert.deepEqual(active.link, { mode: "profile", steamId64: STEAM_ID });

  const [big, manual] = answer.donors;
  assert.equal(big.name, "Крупный");
  assert.equal(big.amount, 15000);
  assert.equal(big.note, "спасибо");
  assert.deepEqual(big.link, { mode: "url", url: "https://example.com/me" });
  assert.equal(manual.profile, null);
  assert.equal(manual.name, "Вручную");
  assert.equal(manual.amount, 300);
  assert.deepEqual(manual.link, { mode: "none" });

  assert.equal(answer.past[0].profile.personaName, "Бывший");
  assert.equal(answer.past[0].since, "2026-01-02");
  assert.equal(answer.past[0].amount, 0);
  assert.deepEqual(request.calls, [V2]);
});

test("v2: порядок сервера сохраняется, лимит 200 на раздел, скрытый ник допустим", async () => {
  const donors = Array.from({ length: 250 }, (_, i) => ({ card: null, name: i === 0 ? "" : `Ник ${i}`, amountRub: 1000 - i, note: "", link: { mode: "none" } }));
  const { instance } = client({ [V2]: reply(listBody({ donors })) });
  const answer = await instance.get();
  assert.equal(answer.donors.length, 200);
  assert.equal(answer.donors[0].name, "", "пустое имя — скрытый ник — остаётся");
  assert.deepEqual(answer.donors.slice(1, 3).map((d) => d.name), ["Ник 1", "Ник 2"]);
  assert.equal(answer.donors[1].amount, 999);
});

test("v2: мусорные записи отбрасываются, остальные остаются", async () => {
  const donors = [
    null, "строка", 5,
    { card: null, name: "Дробный", amountRub: 1.5, note: "", link: { mode: "none" } },
    { card: null, name: "Минус", amountRub: -1, note: "", link: { mode: "none" } },
    { card: null, name: "Строкой", amountRub: "100", note: "", link: { mode: "none" } },
    { card: null, name: 42, amountRub: 100, note: "", link: { mode: "none" } },
    { card: null, name: "я".repeat(65), amountRub: 100, note: "", link: { mode: "none" } },
    { card: null, name: "Заметка", amountRub: 100, note: "я".repeat(161), link: { mode: "none" } },
    { card: null, name: "Хороший", amountRub: 100, note: "", link: { mode: "none" } },
  ];
  const { instance } = client({ [V2]: reply(listBody({ donors })) });
  const answer = await instance.get();
  assert.deepEqual(answer.donors.map((d) => d.name), ["Хороший"]);
});

test("v2: чужой хост аватара отбрасывается, карточка не объект — профиля нет", async () => {
  const body = listBody({
    donors: [
      { card: { personaName: "Чужой", avatarUrl: "https://evil.example/a.png" }, name: "Чужой", amountRub: 10, note: "", link: { mode: "none" } },
      { card: "строка", name: "Без карточки", amountRub: 9, note: "", link: { mode: "none" } },
    ],
  });
  const { instance } = client({ [V2]: reply(body) });
  const answer = await instance.get();
  assert.equal(answer.donors[0].profile.avatarUrl, "");
  assert.equal(answer.donors[1].profile, null);
});

test("ссылка записи: только https до 255 знаков без учётных данных, иначе mode none", async () => {
  const bad = [
    "javascript:alert(1)",
    "http://example.com/",
    "ftp://example.com/",
    "https://user@example.com/",
    "https://user:pass@example.com/",
    "https://example.com:8443/",
    "https://example.com/ пробел",
    "https://example.com/\nline",
    "https://" + "a".repeat(250) + ".com/",
    "not a url",
    "",
    42,
    null,
  ];
  const donors = bad.map((url, i) => ({ card: null, name: `Н${i}`, amountRub: 1, note: "", link: { mode: "url", url } }));
  donors.push({ card: null, name: "Хороший", amountRub: 1, note: "", link: { mode: "url", url: "https://example.com/ok?a=1" } });
  const { instance } = client({ [V2]: reply(listBody({ donors })) });
  const answer = await instance.get();
  assert.equal(answer.donors.length, bad.length + 1, "запись остаётся, пропадает только ссылка");
  for (const entry of answer.donors.slice(0, bad.length)) assert.deepEqual(entry.link, { mode: "none" }, entry.name);
  assert.deepEqual(answer.donors[bad.length].link, { mode: "url", url: "https://example.com/ok?a=1" });
});

test("link.steamId64 проверяется, неверный или неизвестный mode — none", async () => {
  const link = [
    { mode: "profile", steamId64: "7656119800000011" },
    { mode: "profile", steamId64: "76561198000000111x" },
    { mode: "profile", steamId64: "12345678901234567" },
    { mode: "profile", steamId64: 76561198000000111 },
    { mode: "profile" },
    { mode: "weird", url: "https://example.com/" },
    { mode: "url" },
    { url: "https://example.com/" },
    "profile",
    null,
  ];
  const active = link.map((value, i) => ({ card: { personaName: `А${i}` }, since: "2026-10-30", link: value }));
  const { instance } = client({ [V2]: reply(listBody({ active })) });
  const answer = await instance.get();
  assert.equal(answer.active.length, link.length);
  for (const entry of answer.active) assert.deepEqual(entry.link, { mode: "none" }, entry.profile.personaName);
});

test("links: принимаются только https на t.me, tribute.tg и web.tribute.tg", async () => {
  const links = {
    subscribe: "https://web.tribute.tg/s/abc",
    donate: "https://evil.example/tribute",
    bot: "http://t.me/HLSRSupportBot",
    extra: "https://t.me/ignored",
  };
  const { instance } = client({ [V2]: reply(listBody({ links })) });
  const answer = await instance.get();
  assert.deepEqual(answer.links, { subscribe: "https://web.tribute.tg/s/abc" });

  const tricky = {
    subscribe: "https://t.me.evil.example/x",
    donate: "https://nott.me/x",
    bot: "https://user@t.me/x",
  };
  assert.deepEqual((await client({ [V2]: reply(listBody({ links: tricky })) }).instance.get()).links, {});
  for (const bad of ["https://t.me:444/x", "https://tribute.tg.evil.example/", "https://t.me/ x", "https://sub.t.me/x", "javascript:alert(1)", 5, null, ""]) {
    const answer2 = await client({ [V2]: reply(listBody({ links: { donate: bad } })) }).instance.get();
    assert.deepEqual(answer2.links, {}, String(bad));
  }
  assert.deepEqual((await client({ [V2]: reply(listBody({ links: null })) }).instance.get()).links, {});
  assert.deepEqual((await client({ [V2]: reply(listBody({ links: undefined })) }).instance.get()).links, {});
});

test("v2: ответ без трёх разделов — BAD_PAYLOAD, откат на v1 не происходит", async () => {
  for (const body of ['{"sponsors":[]}', "[]", "null", "не json", '{"links":{},"active":[],"donors":[]}']) {
    const { instance, request } = client({ [V2]: reply(body) });
    assert.deepEqual(await instance.get(), { status: "error", version: 2, links: {}, active: [], donors: [], past: [] }, body);
    assert.deepEqual(request.calls, [V2], body);
  }
});

test("404 на /v2 — старый сервер: запрос /v1 и прежний ответ с version 1", async () => {
  const sponsors = [
    { id: "1", name: "Малый", amount: 300, note: "" },
    { id: "2", name: "Крупный", amount: 15000, note: "спасибо", profile: { steamId64: STEAM_ID, personaName: "Big", avatarUrl: "https://evil.example/a.png" } },
    { id: "3", name: "", amount: 100 },
    { id: "4", name: "Дробный", amount: 1.5 },
    { id: "1", name: "Повтор", amount: 99999 },
  ];
  const { instance, request, tick } = client({ [V2]: reply("not found", 404), [V1]: reply({ sponsors }) });
  const answer = await instance.get();
  assert.equal(answer.status, "ok");
  assert.equal(answer.version, 1);
  assert.deepEqual(answer.sponsors.map((s) => s.name), ["Крупный", "Малый"]);
  assert.equal(answer.sponsors[0].profile.steamId64, STEAM_ID);
  assert.equal(answer.sponsors[0].profile.avatarUrl, "");
  assert.deepEqual(request.calls, [V2, V1]);

  tick(4 * 60 * 1000);
  await instance.get();
  assert.equal(request.calls.length, 2, "v1 держится в кеше пять минут");
  tick(61 * 1000);
  await instance.get();
  assert.deepEqual(request.calls.slice(2), [V2, V1], "после пяти минут сервер снова пробуется с /v2");
});

test("v1 после 404: ошибка отдаёт последний список v1 или пустой", async () => {
  const { instance } = client({ [V2]: reply("", 404), [V1]: reply("", 503) });
  assert.deepEqual(await instance.get(), { status: "error", version: 1, sponsors: [] });

  let fail = false;
  const ok = client({ [V2]: reply("", 404), [V1]: () => (fail ? reply("", 500) : reply({ sponsors: [{ id: "1", name: "А", amount: 5 }] })) });
  const first = await ok.instance.get();
  assert.equal(first.sponsors.length, 1);
  fail = true;
  const stale = await ok.instance.get({ force: true });
  assert.equal(stale.status, "error");
  assert.equal(stale.version, 1);
  assert.deepEqual(stale.sponsors.map((s) => s.name), ["А"]);
});

test("ошибка /v2 кроме 404 не откатывается на /v1: ошибка или последний кеш v2", async () => {
  for (const failure of [reply("", 500), reply("", 503), reply("", 403), new Error("ECONNRESET")]) {
    const { instance, request } = client({ [V2]: failure, [V1]: reply({ sponsors: [{ id: "1", name: "А", amount: 5 }] }) });
    assert.deepEqual(await instance.get(), { status: "error", version: 2, links: {}, active: [], donors: [], past: [] });
    assert.deepEqual(request.calls, [V2], "на /v1 идти нельзя");
  }

  let fail = false;
  const { instance, tick } = client({ [V2]: () => (fail ? reply("", 502) : reply(listBody())) });
  const fresh = await instance.get();
  assert.equal(fresh.status, "ok");
  fail = true;
  tick(61 * 1000);
  const stale = await instance.get();
  assert.equal(stale.status, "error");
  assert.equal(stale.version, 2);
  assert.deepEqual(stale.donors, fresh.donors);
  assert.deepEqual(stale.links, fresh.links);
});

test("кеш v2 — минута, force обходит кеш, параллельные запросы делят один", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  let calls = 0;
  const request = async () => { calls++; await gate; return reply(listBody()); };
  let now = 5000;
  const instance = new SponsorsClient({ request, now: () => now });
  const a = instance.get(), b = instance.get(), c = instance.get({ force: true });
  release();
  const [first, second, third] = await Promise.all([a, b, c]);
  assert.equal(calls, 1, "один запрос в полёте");
  assert.equal(first, second);
  assert.equal(second, third);

  now += 59 * 1000;
  await instance.get();
  assert.equal(calls, 1, "в течение минуты берётся кеш");
  await instance.get({ force: true });
  assert.equal(calls, 2, "force идёт на сервер");
  now += 61 * 1000;
  await instance.get();
  assert.equal(calls, 3, "через минуту кеш устарел");
});

test("адрес по умолчанию — /v2 и /v1 на API_BASE, свои адреса QA учитываются", async () => {
  const { instance, request } = client({ [V2]: reply("", 404), [V1]: reply({ sponsors: [] }) });
  await instance.get();
  assert.deepEqual(request.calls, [V2, V1]);

  const qa = "http://127.0.0.1:18895";
  const own = client({
    [qa + "/v2/public/sponsors"]: reply("", 404),
    [qa + "/v1/public/sponsors"]: reply({ sponsors: [] }),
  }, { endpoint: qa + "/v2/public/sponsors", legacyEndpoint: qa + "/v1/public/sponsors" });
  const answer = await own.instance.get();
  assert.equal(answer.version, 1);
  assert.deepEqual(own.request.calls, [qa + "/v2/public/sponsors", qa + "/v1/public/sponsors"]);
});

// ── открытие ссылок ───────────────────────────────────────────────────────

function ipcFixture(routes, confirmed = true) {
  const handlers = new Map(), opened = [], asked = [];
  const { instance, request } = client(routes);
  sponsorsIpc.bind({ handle: (name, fn) => handlers.set(name, fn) }, instance, {
    confirmLink: async (...args) => { asked.push(args); return confirmed; },
    openExternal: async (url) => { opened.push(url); },
  });
  return { invoke: (name, ...args) => handlers.get(name)({}, ...args), opened, asked, client: instance, handlers, request };
}

const INVALID = { status: "error", code: "SPONSOR_LINK_INVALID" };

test("IPC регистрирует sponsor-open-link и support-open", () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  assert.deepEqual([...ipc.handlers.keys()].sort(), ["sponsor-open-link", "support-open"]);
});

test("sponsor-open-link: адрес из последнего списка, не из аргумента, с подтверждением", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  await ipc.client.get();
  const answer = await ipc.invoke("sponsor-open-link", { section: "donors", index: 0, url: "https://evil.example/", href: "https://evil.example/" });
  assert.deepEqual(answer, { status: "ok" });
  assert.deepEqual(ipc.asked, [["example.com", "https://example.com/me"]]);
  assert.deepEqual(ipc.opened, ["https://example.com/me"]);
});

test("sponsor-open-link: отказ в диалоге — адрес не открывается", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) }, false);
  await ipc.client.get();
  assert.deepEqual(await ipc.invoke("sponsor-open-link", { section: "donors", index: 0 }), { status: "cancelled" });
  assert.equal(ipc.asked.length, 1);
  assert.deepEqual(ipc.opened, []);
});

test("sponsor-open-link: чужой раздел и индекс вне списка — ошибка, диалога нет", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  await ipc.client.get();
  const wrong = [
    { section: "donors", index: 2 },
    { section: "donors", index: 99 },
    { section: "donors", index: -1 },
    { section: "donors", index: 0.5 },
    { section: "donors", index: "0" },
    { section: "donors", index: null },
    { section: "donors", index: Infinity },
    { section: "donors" },
    { section: "sponsors", index: 0 },
    { section: "__proto__", index: 0 },
    { section: "constructor", index: 0 },
    { section: "toString", index: 0 },
    { section: ["donors"], index: 0 },
    { index: 0 },
    {},
    null,
    undefined,
    "donors",
    42,
  ];
  for (const argument of wrong) assert.deepEqual(await ipc.invoke("sponsor-open-link", argument), INVALID, JSON.stringify(argument));
  assert.equal(ipc.asked.length, 0);
  assert.equal(ipc.opened.length, 0);
});

test("sponsor-open-link: запись без своего адреса (none, profile) открыть нельзя", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  await ipc.client.get();
  assert.deepEqual(await ipc.invoke("sponsor-open-link", { section: "donors", index: 1 }), INVALID, "mode none");
  assert.deepEqual(await ipc.invoke("sponsor-open-link", { section: "active", index: 0 }), INVALID, "mode profile");
  assert.deepEqual(await ipc.invoke("sponsor-open-link", { section: "past", index: 0 }), INVALID);
  assert.equal(ipc.asked.length, 0);
  assert.equal(ipc.opened.length, 0);
});

test("sponsor-open-link: списка ещё нет или сервер старый (v1) — ошибка", async () => {
  const none = ipcFixture({ [V2]: reply(listBody()) });
  assert.deepEqual(await none.invoke("sponsor-open-link", { section: "donors", index: 0 }), INVALID, "список не запрашивали");

  const old = ipcFixture({ [V2]: reply("", 404), [V1]: reply({ sponsors: [{ id: "1", name: "А", amount: 5 }] }) });
  assert.equal((await old.client.get()).version, 1);
  assert.deepEqual(await old.invoke("sponsor-open-link", { section: "donors", index: 0 }), INVALID);

  const failed = ipcFixture({ [V2]: reply("", 503) });
  await failed.client.get();
  assert.deepEqual(await failed.invoke("sponsor-open-link", { section: "donors", index: 0 }), INVALID);
  assert.equal(none.opened.length + old.opened.length + failed.opened.length, 0);
});

test("sponsor-open-link: адрес перепроверяется перед открытием, даже если список подменён", async () => {
  const handlers = new Map(), opened = [], asked = [];
  const list = { links: {}, active: [], past: [], donors: [
    { profile: null, name: "А", amount: 1, note: "", since: "", link: { mode: "url", url: "javascript:alert(1)" } },
    { profile: null, name: "Б", amount: 1, note: "", since: "", link: { mode: "url", url: "http://example.com/" } },
    { profile: null, name: "В", amount: 1, note: "", since: "", link: { mode: "url", url: "https://u:p@example.com/" } },
  ] };
  sponsorsIpc.bind({ handle: (name, fn) => handlers.set(name, fn) }, { lastList: () => list }, {
    confirmLink: async (...args) => { asked.push(args); return true; },
    openExternal: async (url) => { opened.push(url); },
  });
  for (let index = 0; index < 3; index++) assert.deepEqual(await handlers.get("sponsor-open-link")({}, { section: "donors", index }), INVALID);
  assert.equal(asked.length, 0);
  assert.equal(opened.length, 0);
});

test("sponsor-open-link: сбой диалога или открытия — ошибка, а не исключение", async () => {
  const handlers = new Map();
  const list = { links: {}, active: [], past: [], donors: [{ profile: null, name: "А", amount: 1, note: "", since: "", link: { mode: "url", url: "https://example.com/" } }] };
  sponsorsIpc.bind({ handle: (name, fn) => handlers.set(name, fn) }, { lastList: () => list }, {
    confirmLink: async () => true,
    openExternal: async () => { throw new Error("shell"); },
  });
  assert.deepEqual(await handlers.get("sponsor-open-link")({}, { section: "donors", index: 0 }), INVALID);
});

// Оплата идёт через бота (решение владельца 2026-10-31): бот сначала
// привязывает Telegram к Steam, потом даёт кнопку Tribute. Поэтому
// «Подписка» и «Донат» открывают links.bot с ?start=subscribe|donate, а
// адреса Tribute (links.subscribe, links.donate) лаунчер не открывает.
test("support-open: подписка и донат — бот с ?start=…, bot — сам бот, без диалога", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  await ipc.client.get();
  assert.deepEqual(await ipc.invoke("support-open", "subscribe"), { status: "ok" });
  assert.deepEqual(await ipc.invoke("support-open", "donate"), { status: "ok" });
  assert.deepEqual(await ipc.invoke("support-open", "bot"), { status: "ok" });
  assert.deepEqual(ipc.opened, [
    "https://t.me/HLSRSupportBot?start=subscribe",
    "https://t.me/HLSRSupportBot?start=donate",
    "https://t.me/HLSRSupportBot",
  ], "адреса Tribute из links.subscribe/links.donate не открываются");
  assert.equal(ipc.asked.length, 0, "диалога подтверждения нет");
});

test("support-open: start в адресе бота заменяется, остальные параметры остаются", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody({ links: { bot: "https://t.me/HLSRSupportBot?start=old&ref=hlsr" } })) });
  await ipc.client.get();
  await ipc.invoke("support-open", "donate");
  await ipc.invoke("support-open", "bot");
  assert.deepEqual(ipc.opened, ["https://t.me/HLSRSupportBot?start=donate&ref=hlsr", "https://t.me/HLSRSupportBot?start=old&ref=hlsr"]);
});

test("support-open: бот с кодом привязки — ?start=<6 цифр>; код только у «bot» и только шесть цифр", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody({ links: { bot: "https://t.me/HLSRSupportBot?start=old&ref=hlsr" } })) });
  await ipc.client.get();
  assert.deepEqual(await ipc.invoke("support-open", "bot", "012345"), { status: "ok" });
  assert.deepEqual(ipc.opened, ["https://t.me/HLSRSupportBot?start=012345&ref=hlsr"]);
  for (const [key, code] of [["bot", "12345"], ["bot", "1234567"], ["bot", "12345a"], ["bot", "123456\n"], ["bot", "１２３４５６"], ["bot", 123456], ["bot", null], ["bot", ""],
    ["bot", "123456&start=x"], ["subscribe", "123456"], ["donate", "123456"]]) {
    assert.deepEqual(await ipc.invoke("support-open", key, code), INVALID, JSON.stringify([key, code]));
  }
  assert.equal(ipc.opened.length, 1);
});

test("support-open: неизвестный ключ — ошибка", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  await ipc.client.get();
  for (const key of ["evil", "", "Subscribe", "__proto__", "constructor", "toString", "hasOwnProperty", 0, null, undefined, {}, ["bot"], { key: "bot" }, "https://t.me/x"]) {
    assert.deepEqual(await ipc.invoke("support-open", key), INVALID, JSON.stringify(key));
  }
  assert.equal(ipc.opened.length, 0);
});

test("support-open: без адреса бота — ошибка для всех ключей, ссылки Tribute не помогают; сервер старый — ошибка", async () => {
  const tributeOnly = ipcFixture({ [V2]: reply(listBody({ links: { subscribe: "https://t.me/tribute/app?startapp=s", donate: "https://web.tribute.tg/d" } })) });
  await tributeOnly.client.get();
  for (const key of ["subscribe", "donate", "bot"]) assert.deepEqual(await tributeOnly.invoke("support-open", key), INVALID, key);
  assert.deepEqual(tributeOnly.opened, []);

  const botOnly = ipcFixture({ [V2]: reply(listBody({ links: { bot: "https://t.me/HLSRSupportBot" } })) });
  await botOnly.client.get();
  assert.deepEqual(await botOnly.invoke("support-open", "donate"), { status: "ok" });
  assert.deepEqual(await botOnly.invoke("support-open", "bot"), { status: "ok" });
  assert.deepEqual(botOnly.opened, ["https://t.me/HLSRSupportBot?start=donate", "https://t.me/HLSRSupportBot"]);

  const old = ipcFixture({ [V2]: reply("", 404), [V1]: reply({ sponsors: [] }) });
  await old.client.get();
  assert.deepEqual(await old.invoke("support-open", "bot"), INVALID);
  assert.equal(old.opened.length, 0);
});

test("support-open: адрес бота перепроверяется перед открытием — только https://t.me без учётных данных и порта", async () => {
  const handlers = new Map(), opened = [];
  const list = { links: { bot: "" }, active: [], donors: [], past: [] };
  sponsorsIpc.bind({ handle: (name, fn) => handlers.set(name, fn) }, { lastList: () => list }, {
    confirmLink: async () => true,
    openExternal: async (url) => { opened.push(url); },
  });
  const bad = [
    "https://t.me.evil.example/x", "http://t.me/x", "https://u@t.me/x", "https://u:p@t.me/x", "https://t.me:444/x",
    "https://sub.t.me/x", "https://tribute.tg/x", "https://web.tribute.tg/x", "https://evil.example/", "javascript:alert(1)", "", 5, null,
  ];
  for (const bot of bad) {
    list.links.bot = bot;
    for (const key of ["subscribe", "donate", "bot"]) assert.deepEqual(await handlers.get("support-open")({}, key), INVALID, `${key} ${bot}`);
  }
  assert.equal(opened.length, 0);
});

test("SECTIONS и SUPPORT_KEYS клиента и окна — одни и те же", () => {
  assert.equal(SponsorsClient.SECTIONS, require("../src/shared/sponsor-sections").SECTIONS);
  assert.equal(SponsorsClient.SUPPORT_KEYS, require("../src/shared/sponsor-sections").SUPPORT_KEYS);
});

// ── окно «Спонсорство»: разделы, имена, клики ────────────────────────────

const sections = require("../src/shared/sponsor-sections");

const CARD = { personaName: "Ник", avatarUrl: AVATAR, nick: { style: "gold", css: "" }, avatarFrame: null };

function entry(extra) {
  return Object.assign({ profile: CARD, name: "", amount: 0, note: "", since: "2026-10-30", link: { mode: "none" } }, extra);
}

function v2(extra) {
  return Object.assign({ status: "ok", version: 2, links: {}, active: [], donors: [], past: [] }, extra);
}

test("sectionsOf v2: порядок active, donors, past, пустые разделы выкинуты", () => {
  const active = [entry()], donors = [entry({ name: "Донат", amount: 500 })], past = [entry(), entry()];
  assert.deepEqual(sections.sectionsOf(v2({ active, donors, past })), [
    { key: "active", entries: active },
    { key: "donors", entries: donors },
    { key: "past", entries: past },
  ]);
  assert.deepEqual(sections.sectionsOf(v2({ past, active })).map((s) => s.key), ["active", "past"]);
  assert.deepEqual(sections.sectionsOf(v2({ donors })).map((s) => s.key), ["donors"]);
  assert.deepEqual(sections.sectionsOf(v2()), []);
});

test("sectionsOf v2: записи не фильтруются — номер в разделе совпадает с номером в ответе", () => {
  const donors = [entry({ profile: null, name: "" }), entry({ name: "Второй" }), entry({ link: { mode: "url", url: "https://example.com/" } })];
  const [section] = sections.sectionsOf(v2({ donors }));
  assert.equal(section.entries.length, 3);
  section.entries.forEach((item, index) => assert.equal(item, donors[index]));
});

test("sectionsOf v1: один раздел legacy со списком как есть, даже пустым", () => {
  const sponsors = [{ id: "7", name: "Старый", amount: 50, note: "", profile: null }];
  assert.deepEqual(sections.sectionsOf({ status: "ok", version: 1, sponsors }), [{ key: "legacy", entries: sponsors }]);
  assert.deepEqual(sections.sectionsOf({ status: "error", version: 1, sponsors: [] }), [{ key: "legacy", entries: [] }]);
});

test("sectionsOf: нет ответа, чужая версия или не массивы — пусто", () => {
  for (const answer of [null, undefined, "строка", {}, { status: "ok" }, { version: 3, active: [entry()] }])
    assert.deepEqual(sections.sectionsOf(answer), [], JSON.stringify(answer));
  assert.deepEqual(sections.sectionsOf({ version: 1, sponsors: "нет" }), [{ key: "legacy", entries: [] }]);
  assert.deepEqual(sections.sectionsOf(v2({ active: null, donors: "нет", past: [entry()] })).map((s) => s.key), ["past"]);
});

test("isLegacy: старый сервер (version 1) — разметка 0.4.7", () => {
  assert.equal(sections.isLegacy({ status: "ok", version: 1, sponsors: [] }), true);
  assert.equal(sections.isLegacy(v2()), false);
  assert.equal(sections.isLegacy(null), false, "без ответа — новое окно с сообщением об ошибке");
});

test("entryName: имя записи, иначе ник профиля; пустое — аноним", () => {
  assert.equal(sections.entryName(entry({ name: "", profile: { ...CARD, personaName: "Подписчик" } })), "Подписчик");
  assert.equal(sections.entryName(entry({ name: "Вручную", profile: null })), "Вручную");
  assert.equal(sections.entryName(entry({ name: "Из админки", profile: { ...CARD, personaName: "Steam-ник" } })), "Из админки");
  assert.equal(sections.entryName(entry({ name: "", profile: null })), "");
  assert.equal(sections.entryName(entry({ name: "", profile: { ...CARD, personaName: "" } })), "");
  assert.equal(sections.entryName(entry({ name: "   ", profile: { ...CARD, personaName: " " } })), "", "одни пробелы — тоже аноним");
  assert.equal(sections.entryName(null), "");
});

test("entrySubscriber: значок подписчика — при subscriber: true и видимом нике, у анонима нет", () => {
  assert.equal(sections.entrySubscriber(entry({ profile: { ...CARD, subscriber: true } })), true);
  assert.equal(sections.entrySubscriber(entry({ name: "Из админки", profile: { ...CARD, subscriber: true } })), true);
  assert.equal(sections.entrySubscriber(entry({ profile: { ...CARD, subscriber: false } })), false);
  assert.equal(sections.entrySubscriber(entry({ profile: CARD })), false, "без ключа");
  assert.equal(sections.entrySubscriber(entry({ profile: { ...CARD, subscriber: "true" } })), false);
  assert.equal(sections.entrySubscriber(entry({ name: "Вручную", profile: null })), false, "без карточки");
  assert.equal(sections.entrySubscriber(entry({ name: "", profile: { ...CARD, personaName: "", subscriber: true } })), false, "скрытый ник — без значка");
  const { instance } = loadModal(v2());
  assert.equal(instance.subscriberOf(entry({ profile: { ...CARD, subscriber: true } })), true, "окно берёт то же правило");
  assert.equal(instance.subscriberOf(entry({ name: "", profile: { ...CARD, personaName: "", subscriber: true } })), false);
});

test("карточки v2 и профиль v1: subscriber только при true, иначе false", async () => {
  const body = listBody({
    active: [
      { card: { personaName: "Подписчик", subscriber: true }, since: "2026-10-30", link: { mode: "none" } },
      { card: { personaName: "Строкой", subscriber: "true" }, since: "2026-10-30", link: { mode: "none" } },
      { card: { personaName: "Единицей", subscriber: 1 }, since: "2026-10-30", link: { mode: "none" } },
    ],
    donors: [{ card: { personaName: "Донат", subscriber: true }, name: "Донат", amountRub: 500, note: "", link: { mode: "none" } }],
    past: [{ card: { personaName: "Бывший" }, since: "2026-01-02", link: { mode: "none" } }],
  });
  const answer = await client({ [V2]: reply(body) }).instance.get();
  assert.deepEqual(answer.active.map((item) => item.profile.subscriber), [true, false, false]);
  assert.equal(answer.donors[0].profile.subscriber, true);
  assert.equal(answer.past[0].profile.subscriber, false);

  const sponsors = [
    { id: "1", name: "Подписчик", amount: 300, note: "", profile: { steamId64: STEAM_ID, personaName: "Sub", subscriber: true } },
    { id: "2", name: "Строкой", amount: 200, note: "", profile: { steamId64: STEAM_ID, personaName: "Str", subscriber: "true" } },
  ];
  const legacy = await client({ [V2]: reply("not found", 404), [V1]: reply({ sponsors }) }).instance.get();
  assert.deepEqual(legacy.sponsors.map((item) => item.profile.subscriber), [true, false]);
});

test("entryAction: профиль по link.steamId64, свой адрес, иначе не кликается", () => {
  assert.equal(sections.entryAction(entry({ link: { mode: "profile", steamId64: STEAM_ID } })), "profile");
  assert.equal(sections.entryAction(entry({ link: { mode: "url", url: "https://example.com/me" } })), "url");
  assert.equal(sections.entryAction(entry({ link: { mode: "none" } })), null);
  assert.equal(sections.entryAction(entry({ link: { mode: "profile" } })), null, "профиль без SteamID не открыть");
  assert.equal(sections.entryAction(entry({ link: { mode: "url" } })), null, "адреса нет — ссылки нет");
  assert.equal(sections.entryAction(entry({ link: null })), null);
  assert.equal(sections.entryAction(null), null);
});

test("supportKeys: «Подписка» и «Донат» — только при адресе бота (оплата через бота), кнопки «Бот» в окне нет", () => {
  assert.deepEqual(sections.supportKeys({ bot: "https://t.me/b", subscribe: "https://t.me/s", donate: "https://t.me/d" }), ["subscribe", "donate"]);
  assert.deepEqual(sections.supportKeys({ bot: "https://t.me/b" }), ["subscribe", "donate"]);
  assert.deepEqual(sections.supportKeys({ subscribe: "https://t.me/s", donate: "https://t.me/d" }), [], "ссылки Tribute без бота — кнопок нет");
  assert.deepEqual(sections.supportKeys({ bot: "", other: "https://t.me/x" }), []);
  assert.deepEqual(sections.supportKeys(undefined), []);
});

test("sponsorTier: трон, пьедестал (2-е и 3-е), остальные", () => {
  assert.deepEqual([0, 1, 2, 3, 10].map(sections.sponsorTier), ["king", "podium", "podium", "rest", "rest"]);
});

test("amountShort: до миллиона полностью, дальше коротко; amountText — всегда полностью", () => {
  const group = (n, locale) => n.toLocaleString(locale);
  assert.equal(sections.amountText(15000, "ru-RU"), group(15000, "ru-RU") + " ₽");
  assert.equal(sections.amountShort(15000, "ru-RU"), group(15000, "ru-RU") + " ₽");
  assert.equal(sections.amountShort(999999, "en-GB"), "999,999 ₽");
  assert.match(sections.amountShort(2500000, "en-GB"), /^2\.5[mM] ₽$/);
  assert.equal(sections.amountText(2500000, "en-GB"), "2,500,000 ₽");
  assert.match(sections.amountShort(100000000, "ru-RU"), /^100\sмлн ₽$/);
});

// Компонент без Vue: скрипт SFC в vm, require — относительно файла окна.
// Объекты из vm — другого «мира» (свои Object и Array), deepStrictEqual их
// не сравнит с местными: значения из окна сравниваются через plain().
function plain(value) {
  return JSON.parse(JSON.stringify(value));
}

function loadModal(answer) {
  const fs = require("node:fs"), vm = require("node:vm"), path = require("node:path");
  const { parse } = require("vue/compiler-sfc");
  const file = path.join(__dirname, "../src/renderer/components/Launcher/SponsorsModal.vue");
  const script = parse(fs.readFileSync(file, "utf8")).descriptor.script.content;
  const calls = [];
  const context = {
    module: { exports: {} }, NickName: {}, AvatarFrame: {}, trapFocus: () => {}, uiState: {},
    require: (id) => require(id.startsWith(".") ? path.join(path.dirname(file), id) : id),
    window: { hlsrElectron: { ipcRenderer: { invoke: async (channel, ...args) => { calls.push(plain([channel, ...args])); return channel === "public-sponsors-get" ? answer : { status: "ok" }; } } } },
  };
  vm.runInNewContext(script.replace(/^import .*;\s*$/gm, "").replace(/^export (function|const)/gm, "$1").replace("export default", "module.exports ="), context);
  const component = context.module.exports;
  const emitted = [];
  const instance = Object.assign(component.data(), { $emit: (...args) => emitted.push(args) });
  for (const [name, fn] of Object.entries(component.methods)) instance[name] = fn.bind(instance);
  for (const [name, fn] of Object.entries(component.computed || {})) Object.defineProperty(instance, name, { get: fn.bind(instance) });
  return { component, instance, calls, emitted };
}

test("окно: список запрашивается один раз при открытии", async () => {
  const answer = v2({ links: { bot: "https://t.me/b" }, active: [entry()] });
  const { component, instance, calls } = loadModal(answer);
  assert.equal(instance.loading, true);
  await component.created.call(instance);
  assert.deepEqual(calls, [["public-sponsors-get"]]);
  assert.equal(instance.loading, false);
  assert.equal(instance.failed, false);
  assert.equal(instance.legacy, false);
  assert.deepEqual(instance.sections.map((s) => s.key), ["active"]);
  assert.deepEqual(instance.supportKeys, ["subscribe", "donate"]);
});

test("окно: пока список грузится — только загрузка, без шапки нового окна (старый сервер не мигает)", async () => {
  for (const [answer, view] of [[v2(), "window"], [{ status: "ok", version: 1, sponsors: [] }, "legacy"], [v2({ status: "error" }), "window"], [null, "window"]]) {
    const { component, instance } = loadModal(answer);
    assert.equal(instance.view, "loading", "до ответа");
    await component.created.call(instance);
    assert.equal(instance.view, view, JSON.stringify(answer));
  }
});

test("окно: старый сервер — разметка 0.4.7, без кнопок; ошибка и сбой IPC — failed", async () => {
  const v1 = [{ id: "7", name: "Старый", amount: 50, note: "", profile: null }];
  const old = loadModal({ status: "ok", version: 1, sponsors: v1 });
  await old.component.created.call(old.instance);
  assert.equal(old.instance.legacy, true);
  assert.deepEqual(old.instance.sponsors, v1);
  assert.deepEqual(plain(old.instance.supportKeys), []);
  assert.equal(old.instance.failed, false);

  const failed = loadModal(v2({ status: "error" }));
  await failed.component.created.call(failed.instance);
  assert.equal(failed.instance.failed, true);
  assert.deepEqual(plain(failed.instance.sections), []);

  const broken = loadModal(null);
  await broken.component.created.call(broken.instance);
  assert.equal(broken.instance.failed, true);
  assert.equal(broken.instance.legacy, false);
});

test("окно: клик — профиль по link.steamId64, свой адрес через main по номеру в разделе, none — ничего", async () => {
  const donors = [
    entry({ name: "Без ссылки" }),
    entry({ name: "Профиль", link: { mode: "profile", steamId64: STEAM_ID } }),
    entry({ name: "Сайт", link: { mode: "url", url: "https://example.com/me" } }),
  ];
  const past = [entry({ link: { mode: "url", url: "https://example.org/" } })];
  const { component, instance, calls, emitted } = loadModal(v2({ donors, past }));
  await component.created.call(instance);
  calls.length = 0;

  instance.activate("donors", 0);
  assert.deepEqual(emitted, []);
  assert.deepEqual(calls, []);

  instance.activate("donors", 1);
  assert.deepEqual(emitted, [["open-profile", STEAM_ID]]);

  instance.activate("donors", 2);
  instance.activate("past", 0);
  assert.deepEqual(calls, [["sponsor-open-link", { section: "donors", index: 2 }], ["sponsor-open-link", { section: "past", index: 0 }]]);
  assert.equal(emitted.length, 1, "адрес не открывает профиль");

  instance.activate("active", 5);
  instance.activate("legacy", 0);
  assert.equal(calls.length, 2, "чужой раздел или номер — ничего");
});

test("окно: «Подписка» и «Донат» зовут support-open с ключом; без бота и для «bot» — ничего", async () => {
  const { component, instance, calls } = loadModal(v2({ links: { bot: "https://t.me/b", subscribe: "https://t.me/s" } }));
  await component.created.call(instance);
  calls.length = 0;
  instance.support("donate");
  instance.support("subscribe");
  instance.support("bot");
  assert.deepEqual(calls, [["support-open", "donate"], ["support-open", "subscribe"]], "кнопки «Бот» в окне нет — и вызова нет");

  const tribute = loadModal(v2({ links: { subscribe: "https://t.me/s", donate: "https://t.me/d" } }));
  await tribute.component.created.call(tribute.instance);
  tribute.calls.length = 0;
  tribute.instance.support("subscribe");
  tribute.instance.support("donate");
  assert.deepEqual(tribute.calls, [], "без адреса бота кнопок нет");
});

test("локали: ключи окна есть в ru и en, неиспользуемых #L_SPONSORS_* нет, подсказка про оплату через бота", () => {
  const fs = require("node:fs"), path = require("node:path");
  const read = (file) => fs.readFileSync(path.join(__dirname, "..", file), "utf8");
  const keysOf = (text) => new Set([...text.matchAll(/^\s*"(#L_[A-Z0-9_]+)":/gm)].map((m) => m[1]));
  const ru = read("src/renderer/utils/locales/ru.locale.js"), en = read("src/renderer/utils/locales/en.locale.js");
  const ruKeys = keysOf(ru), enKeys = keysOf(en);
  const used = new Set(read("src/renderer/components/Launcher/SponsorsModal.vue").match(/#L_S(?:PONSORS)?_[A-Z0-9_]+/g));
  for (const key of used) assert.ok(ruKeys.has(key) && enKeys.has(key), key);
  for (const key of [...ruKeys, ...enKeys].filter((k) => k.startsWith("#L_SPONSORS_"))) assert.ok(used.has(key), `не используется: ${key}`);
  assert.ok(!ruKeys.has("#L_SPONSORS_BOT") && !enKeys.has("#L_SPONSORS_BOT"), "кнопки «Привязать Telegram» в окне больше нет");
  assert.ok(ru.includes("\"#L_SPONSORS_HINT\": \"Не ставьте галочку «анонимно» в Tribute.\""));
  assert.ok(en.includes("\"#L_SPONSORS_HINT\": \"Don't tick «anonymous» in Tribute.\""));
  // Пояснение окна — текст владельца; «<3» — буквы, а не разметка: окно
  // выводит его интерполяцией, без v-html.
  assert.ok(ru.includes("\"#L_SPONSORS_ABOUT\":\n    \"Поддержите лаунчер HLSR ежемесячной подпиской по цене пары пачек «Кириешек» или единоразовым донатом! Так вы сможете поддержать нашу работу. Спасибо! <3\","));
  assert.ok(en.includes("\"#L_SPONSORS_ABOUT\":\n    \"Support the HLSR launcher with a monthly subscription for the price of a couple of snack packs, or with a one-time donation! It helps us keep working on HLSR. Thank you! <3\","));
  const modal = read("src/renderer/components/Launcher/SponsorsModal.vue");
  assert.ok(modal.includes("<p class=\"sponsors-modal__about\">{{ $t(\"#L_SPONSORS_ABOUT\") }}</p>"));
  assert.ok(!/v-html/.test(modal), "окно не вставляет HTML");
});

// ── кнопки бота без открытого окна «Спонсорство» ─────────────────────────
// Кнопка «Привязать» в настройках профиля зовёт support-open("bot"), когда
// окно спонсоров могли ни разу не открывать: main сначала запрашивает список.

test("support-open: списка ещё нет (окно «Спонсорство» не открывали) — main сначала запрашивает его, потом открывает бота", async () => {
  const ipc = ipcFixture({ [V2]: reply(listBody()) });
  assert.equal(ipc.client.lastList(), null);
  assert.deepEqual(await ipc.invoke("support-open", "bot"), { status: "ok" });
  assert.deepEqual(ipc.opened, ["https://t.me/HLSRSupportBot"]);
  assert.deepEqual(ipc.request.calls, [V2]);
  assert.deepEqual(await ipc.invoke("support-open", "donate"), { status: "ok" });
  assert.deepEqual(ipc.request.calls, [V2], "список уже есть — второй раз не запрашивается");

  // Сервер старый (v1), /v2 сбоит или сети нет — после запроса списка бота всё равно нет.
  for (const routes of [{ [V2]: reply("", 404), [V1]: reply({ sponsors: [] }) }, { [V2]: reply("", 503) }, { [V2]: new Error("offline") }]) {
    const failed = ipcFixture(routes);
    assert.deepEqual(await failed.invoke("support-open", "bot"), INVALID);
    assert.equal(failed.request.calls[0], V2, "список запрошен");
    assert.deepEqual(failed.opened, []);
  }
});

test("links.bot: только адрес, который откроет support-open (https://t.me), иначе бота нет — и кнопок оплаты тоже", async () => {
  for (const bot of ["https://tribute.tg/HLSRSupportBot", "https://web.tribute.tg/bot"]) {
    const answer = await client({ [V2]: reply(listBody({ links: { bot, subscribe: "https://web.tribute.tg/s" } })) }).instance.get();
    assert.deepEqual(answer.links, { subscribe: "https://web.tribute.tg/s" }, bot);
    assert.deepEqual(sections.supportKeys(answer.links), [], "кнопки не показываются, раз бот не откроется");
  }
  const ok = await client({ [V2]: reply(listBody({ links: { bot: "https://t.me/HLSRSupportBot?start=x" } })) }).instance.get();
  assert.deepEqual(ok.links, { bot: "https://t.me/HLSRSupportBot?start=x" });
});

// ── заготовки «подсветка ника + рамка аватара» ───────────────────────────

const StylePresetsClient = require("../src/main/telemetry/style-presets-client");
const PRESETS = "https://api.hlsr.pro/v2/public/style-presets";

function presetsClient(routes, options) {
  const request = fakeRequest(routes);
  let now = 1000000;
  const instance = new StylePresetsClient(Object.assign({ request, now: () => now }, options));
  return { instance, request, tick: (ms) => { now += ms; } };
}

const preset = (extra) => Object.assign({ id: 1, title: "А", nickCss: "color: red", frameCss: "" }, extra);

test("заготовки: CSS ника и рамки проходит те же проверки, что оформление профиля; не прошедшая заготовка не показывается", async () => {
  const body = [
    { id: 3, title: " Золото ", nickCss: "color: #ffd700; text-shadow: 0 0 6px #ffae00", frameCss: "border: 2px solid #ffd700" },
    { id: 4, title: "Только рамка", nickCss: "", frameCss: "box-shadow: 0 0 8px #0ff" },
    { id: 5, title: "Сеть в нике", nickCss: "background-image: url(https://evil.example/x.png)", frameCss: "" },
    { id: 6, title: "expression", nickCss: "color: expression(alert(1))", frameCss: "" },
    { id: 7, title: "Сеть в рамке", nickCss: "color: red", frameCss: "background: url(x.png)" },
    { id: 8, title: "Размер в нике", nickCss: "position: absolute; width: 900px", frameCss: "" },
    { id: 9, title: "Свойство ника в рамке", nickCss: "", frameCss: "-webkit-text-fill-color: red" },
    { id: 10, title: "Пусто после проверки", nickCss: ";", frameCss: "" },
    { id: 11, title: "Только ник", nickCss: "color: #f0f", frameCss: null },
  ];
  const answer = await presetsClient({ [PRESETS]: reply(body) }).instance.get();
  assert.equal(answer.status, "ok");
  assert.deepEqual(answer.presets, [
    { id: 3, title: "Золото", nick: { style: null, css: "color: #ffd700; text-shadow: 0 0 6px #ffae00" }, frame: { css: "border: 2px solid #ffd700" } },
    { id: 4, title: "Только рамка", nick: { style: null, css: "" }, frame: { css: "box-shadow: 0 0 8px #0ff" } },
    { id: 11, title: "Только ник", nick: { style: null, css: "color: #f0f" }, frame: { css: "" } },
  ]);
});

test("заготовки: неверный номер, название, CSS не строкой и повтор — отбрасываются; не массив — ошибка", async () => {
  const body = [
    preset({ id: 1 }), preset({ id: 1, title: "Повтор" }), preset({ id: 0 }), preset({ id: -2 }), preset({ id: 1.5 }), preset({ id: "2" }),
    preset({ id: 2 ** 32 }), preset({ id: 12, title: "" }), preset({ id: 13, title: "   " }), preset({ id: 14, title: "x".repeat(49) }),
    preset({ id: 15, title: "a\nb" }), preset({ id: 16, title: 5 }), preset({ id: 17, nickCss: 5 }), preset({ id: 18, frameCss: {} }),
    null, "x", 5, [], preset({ id: 19, title: "ё".repeat(48) }), preset({ id: 2 ** 32 - 1 }),
  ];
  const answer = await presetsClient({ [PRESETS]: reply(body) }).instance.get();
  assert.deepEqual(answer.presets.map((item) => item.id), [1, 19, 2 ** 32 - 1]);
  assert.equal(answer.presets[0].title, "А", "из повторов остаётся первый");

  for (const bad of ['{"presets":[]}', "{}", "null", "не json", "5"]) {
    assert.deepEqual(await presetsClient({ [PRESETS]: reply(bad) }).instance.get(), { status: "error", presets: [] }, bad);
  }
});

test("заготовки: 404 (сервер на PHP) — пустой список без ошибки; сбой — ошибка и последний полученный список", async () => {
  const old = presetsClient({});
  assert.deepEqual(await old.instance.get(), { status: "ok", presets: [] });
  assert.deepEqual(old.request.calls, [PRESETS]);

  let route = reply([preset({ id: 3, title: "Золото" })]);
  const { instance, tick } = presetsClient({ [PRESETS]: () => route });
  assert.deepEqual((await instance.get()).presets.map((item) => item.id), [3]);
  tick(6 * 60 * 1000);
  for (const failure of [reply("", 500), reply("", 503), new Error("offline")]) {
    route = failure;
    const answer = await instance.get({ force: true });
    assert.equal(answer.status, "error");
    assert.deepEqual(answer.presets.map((item) => item.id), [3], "последний полученный список остаётся");
  }
  assert.deepEqual(await presetsClient({ [PRESETS]: reply("", 500) }).instance.get(), { status: "error", presets: [] });
});

test("заготовки: кеш 5 минут, force обходит кеш, параллельные запросы делят один", async () => {
  let calls = 0;
  const { instance, tick } = presetsClient({ [PRESETS]: () => { calls++; return reply([preset()]); } });
  await Promise.all([instance.get(), instance.get()]);
  assert.equal(calls, 1);
  tick(299 * 1000);
  await instance.get();
  assert.equal(calls, 1);
  tick(2 * 1000);
  await instance.get();
  assert.equal(calls, 2, "через 5 минут кеш устарел");
  await instance.get({ force: true });
  assert.equal(calls, 3);
  const old = presetsClient({});
  await old.instance.get();
  await old.instance.get();
  assert.equal(old.request.calls.length, 1, "ответ старого сервера тоже кешируется");
});

test("заготовки: адрес по умолчанию — /v2/public/style-presets на API_BASE, адрес QA учитывается", async () => {
  const { instance, request } = presetsClient({ [PRESETS]: reply([]) });
  await instance.get();
  assert.deepEqual(request.calls, [PRESETS]);
  const qa = "http://127.0.0.1:18895/v2/public/style-presets";
  const own = presetsClient({ [qa]: reply([]) }, { endpoint: qa });
  await own.instance.get();
  assert.deepEqual(own.request.calls, [qa]);
});

// ── профиль: выбор подсветки и Telegram (компоненты без Vue) ─────────────

function loadComponent(name, props, answers) {
  const fs = require("node:fs"), vm = require("node:vm"), path = require("node:path");
  const { parse } = require("vue/compiler-sfc");
  const file = path.join(__dirname, "../src/renderer/components/Launcher/" + name + ".vue");
  const script = parse(fs.readFileSync(file, "utf8")).descriptor.script.content;
  const calls = [];
  // Таймеры не идут сами: тест вызывает fn, когда нужно.
  const timers = [];
  const context = {
    module: { exports: {} }, NickName: {}, AvatarFrame: {}, UiDropdown: {}, RunPlace: {}, StylePresetPicker: {}, formatRun: () => "", OFFICIAL: true,
    setTimeout: (fn, ms) => timers.push({ fn, ms }), clearTimeout: () => {},
    require: (id) => require(id.startsWith(".") ? path.join(path.dirname(file), id) : id),
    window: { hlsrElectron: { ipcRenderer: { invoke: async (channel, ...args) => {
      calls.push(plain([channel, ...args]));
      const answer = (answers || {})[channel];
      const value = typeof answer === "function" ? await answer(...args) : answer;
      if (value instanceof Error) throw value;
      return value;
    } } } },
  };
  vm.runInNewContext(script.replace(/^import .*;\s*$/gm, "").replace(/^export (function|const)/gm, "$1").replace("export default", "module.exports ="), context);
  const component = context.module.exports;
  const emitted = [];
  const instance = Object.assign(component.data ? component.data() : {}, props, {
    $emit: (...args) => emitted.push(plain(args)),
    $t: (key, ...args) => [key, ...args].join("|"),
  });
  for (const [key, fn] of Object.entries(component.methods || {})) instance[key] = fn.bind(instance);
  for (const [key, fn] of Object.entries(component.computed || {})) Object.defineProperty(instance, key, { get: fn.bind(instance) });
  return { component, instance, calls, emitted, timers };
}

const PRESET_LIST = [
  { id: 3, title: "Золото", nick: { style: null, css: "color: #ffd700" }, frame: { css: "border: 2px solid #ffd700" } },
  { id: 4, title: "Неон", nick: { style: null, css: "color: #0ff" }, frame: { css: "" } },
];
const NO_PERKS = { styles: false, subscriber: null };
const STYLES = { styles: true, subscriber: null };
const NOT_LINKED = { linked: false, username: null };

function picker(props, answers) {
  return loadComponent("StylePresetPicker", Object.assign({ perks: STYLES, current: null, presets: PRESET_LIST, account: { personaName: "Runner" } }, props), answers);
}

test("подсветка: с правом — сетка «Без подсветки» и заготовок, выбранная отмечена", () => {
  const { instance } = picker({ current: 4 });
  assert.equal(instance.mode, "grid");
  assert.deepEqual(plain(instance.options.map((option) => option.id)), [0, 3, 4]);
  assert.equal(instance.options[0].title, "#L_RUNNER_STYLE_NONE");
  assert.equal(instance.selected, 4);
  assert.equal(picker({ current: null }).instance.selected, 0, "без заготовки отмечена «Без подсветки»");
  assert.equal(picker({ current: 99 }).instance.selected, null, "скрытую или удалённую заготовку отметить нечем");
});

test("подсветка: выбор уходит в runner-customize, снимок профиля — наверх, «сохранено»", async () => {
  const snapshot = { account: { signedIn: true }, stylePreset: 3 };
  const { instance, calls, emitted } = picker({}, { "runner-customize": { status: "ok", snapshot } });
  await instance.choose(3);
  assert.deepEqual(calls, [["runner-customize", { stylePreset: 3 }]]);
  assert.deepEqual(emitted, [["saved", snapshot]]);
  assert.equal(instance.message, "#L_RUNNER_STYLE_SAVED");
  assert.equal(instance.tone, "ok");
  assert.equal(instance.pending, null);

  instance.current = 3;
  await instance.choose(0);
  assert.deepEqual(calls[1], ["runner-customize", { stylePreset: 0 }], "«Без подсветки» — stylePreset 0");
  instance.current = null;
  await instance.choose(0);
  assert.equal(calls.length, 2, "уже выбранное не отправляется");
  await instance.choose(77);
  assert.equal(calls.length, 2, "заготовки нет в списке — не отправляется");
});

test("подсветка: отказ — свой текст: нет права, заготовка исчезла (список перечитывается), сбой сервера и сети", async () => {
  let result;
  const { instance, emitted } = picker({}, { "runner-customize": () => result });
  const cases = [
    [{ status: "error", code: "RUNNER_PERK_REQUIRED" }, "#L_RUNNER_STYLE_PERK_REQUIRED"],
    [{ status: "error", code: "RUNNER_PRESET_UNKNOWN" }, "#L_RUNNER_STYLE_UNKNOWN"],
    [{ status: "error", code: "RUNNER_HTTP_500" }, "#L_RUNNER_STYLE_ERROR"],
    [{ status: "error", code: "RUNNER_NETWORK" }, "#L_RUNNER_NETWORK"],
    [null, "#L_RUNNER_STYLE_ERROR"],
    [new Error("ipc"), "#L_RUNNER_NETWORK"],
  ];
  for (const [answer, message] of cases) {
    result = answer;
    await instance.choose(3);
    assert.equal(instance.message, message, JSON.stringify(answer && answer.code));
    assert.equal(instance.tone, "error");
    assert.equal(instance.pending, null);
  }
  assert.deepEqual(emitted, [["stale"]], "снимка нет; список перечитывается только когда заготовка исчезла");
});

test("подсветка: пока выбор сохраняется, второй не уходит", async () => {
  let finish;
  const { instance, calls } = picker({}, { "runner-customize": () => new Promise((resolve) => { finish = resolve; }) });
  const first = instance.choose(3);
  assert.equal(instance.pending, 3);
  await instance.choose(4);
  assert.equal(calls.length, 1);
  finish({ status: "ok", snapshot: { stylePreset: 3 } });
  await first;
  assert.equal(instance.pending, null);
});

test("подсветка: без права — пояснение и «Поддержать проект» (окно «Спонсорство»); заготовку не выбрать, снять старую — можно", async () => {
  const { instance, calls, emitted } = picker({ perks: { styles: false, subscriber: { active: false, until: "2026-10-01T12:00:00Z" } }, current: 3 },
    { "runner-customize": { status: "ok", snapshot: { stylePreset: null } } });
  assert.equal(instance.mode, "locked");
  instance.support();
  assert.deepEqual(emitted, [["open-sponsors"]]);
  await instance.choose(4);
  assert.deepEqual(calls, [], "без права заготовка не выбирается");
  await instance.choose(0);
  assert.deepEqual(calls, [["runner-customize", { stylePreset: 0 }]], "снять заготовку можно и без права");
});

test("подсветка: пустой список заготовок (сервер на PHP) — ни сетки, ни пояснения", () => {
  assert.equal(picker({ presets: [] }).instance.mode, "none");
  assert.equal(picker({ presets: [], perks: NO_PERKS }).instance.mode, "none");
});

test("подсветка: заготовок нет и строки подписки нет — блока нет совсем (ни отступов, ни разделителя)", () => {
  assert.equal(picker({ presets: [] }).instance.visible, false, "право есть, заготовок нет");
  assert.equal(picker({ presets: [], perks: NO_PERKS }).instance.visible, false);
  assert.equal(picker({ presets: [], perks: { styles: false, subscriber: { active: false, until: null } } }).instance.visible, true, "строка подписки остаётся");
  assert.equal(picker({}).instance.visible, true, "сетка");
  assert.equal(picker({ perks: NO_PERKS }).instance.visible, true, "пояснение без права");
  const fs = require("node:fs"), path = require("node:path");
  const { parse } = require("vue/compiler-sfc");
  const template = parse(fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/StylePresetPicker.vue"), "utf8")).descriptor.template.content;
  assert.match(template.trim(), /^<div v-if="visible" class="style-picker">/, "корень рисуется только при visible");
});

// Дата конца подписки — день по часам игрока (как «В HLSR с …» в профиле):
// ожидание строится теми же местными геттерами, поэтому тест не зависит от
// пояса машины. Чтобы это проверялось, он прогоняется и в UTC+14, и в UTC−12
// (process.env.TZ меняет пояс процесса, в конце пояс возвращается).
function localDay(iso) {
  const date = new Date(iso);
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
}

test("подсветка: строка подписки — активна до ДД.ММ.ГГГГ, активна без даты, закончилась; подписки не было — строки нет", () => {
  const line = (subscriber) => picker({ perks: { styles: true, subscriber } }).instance.subscriberText;
  const system = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const days = {};
  try {
    for (const zone of [system, "Pacific/Kiritimati", "Etc/GMT+12"]) {
      process.env.TZ = zone;
      for (const until of ["2026-11-30T12:00:00Z", "2026-11-30T23:30:00Z", "2026-12-01T00:30:00Z"])
        assert.equal(line({ active: true, until }), "#L_RUNNER_STYLE_SUB_UNTIL|" + localDay(until), `${zone} ${until}`);
      days[zone] = localDay("2026-11-30T12:00:00Z");
    }
  } finally {
    process.env.TZ = system;
  }
  assert.deepEqual([days["Pacific/Kiritimati"], days["Etc/GMT+12"]], ["01.12.2026", "30.11.2026"], "пояс в тесте действительно менялся");
  assert.equal(line({ active: true, until: null }), "#L_RUNNER_STYLE_SUB_ACTIVE");
  assert.equal(line({ active: false, until: "2026-10-01T12:00:00Z" }), "#L_RUNNER_STYLE_SUB_ENDED");
  assert.equal(line({ active: false, until: null }), "#L_RUNNER_STYLE_SUB_ENDED");
  assert.equal(line(null), null);
});

function settings(props, answers) {
  return loadComponent("RunnerSettings", Object.assign({ perks: NO_PERKS, stylePreset: null, telegram: NOT_LINKED, account: {}, details: {} }, props), answers);
}

test("профиль: заготовки запрашиваются при открытии настроек, «исчезла» — перечитываются мимо кеша", async () => {
  const { component, instance, calls } = settings({}, { "public-style-presets-get": { status: "ok", presets: PRESET_LIST } });
  await component.created.call(instance);
  assert.deepEqual(calls, [["public-style-presets-get"]]);
  assert.deepEqual(plain(instance.stylePresets), PRESET_LIST);
  await instance.loadPresets(true);
  assert.deepEqual(calls[1], ["public-style-presets-get", { force: true }]);
});

test("профиль: группа видна при заготовках, праве, подписке или привязанном Telegram; сервер на PHP — группы нет", async () => {
  const visible = async (props, answer) => {
    const { component, instance } = settings(props, { "public-style-presets-get": answer });
    await component.created.call(instance);
    return instance.styleGroup;
  };
  const none = { status: "ok", presets: [] };
  assert.equal(await visible({}, { status: "ok", presets: PRESET_LIST }), true);
  assert.equal(await visible({}, none), false, "сервер на PHP: 404 — пустой список");
  assert.equal(await visible({}, { status: "error", presets: [] }), false);
  assert.equal(await visible({}, new Error("ipc")), false);
  assert.equal(await visible({ telegram: { linked: true, username: null } }, none), true);
  assert.equal(await visible({ perks: STYLES }, none), true);
  assert.equal(await visible({ perks: { styles: false, subscriber: { active: false, until: null } } }, none), true);
});

test("профиль: Telegram — «привязан (@имя)», без имени, «не привязан»", () => {
  assert.equal(settings({ telegram: { linked: true, username: "hlsr_fan" } }).instance.telegramLabel, "#L_RUNNER_TELEGRAM_LINKED_AS|@hlsr_fan");
  assert.equal(settings({ telegram: { linked: true, username: null } }).instance.telegramLabel, "#L_RUNNER_TELEGRAM_LINKED");
  assert.equal(settings({}).instance.telegramLabel, "#L_RUNNER_TELEGRAM_NOT_LINKED");
});

// Аудит S-7: бот привязывает Telegram только по одноразовому коду, который
// сервер выдаёт своему аккаунту. «Привязать» берёт код (runner-telegram-code)
// и открывает бота с ним (support-open "bot" <код> → ?start=<код>).
test("профиль: «Привязать» — код с сервера, затем бот с этим кодом; пока код жив, новый не просится", async () => {
  const expiresAt = Math.floor(Date.now() / 1000) + 600;
  let code = { code: "123456", expiresAt, profile: {}, snapshot: { account: { signedIn: true } } };
  let open = { status: "ok" };
  let sync;
  const { component, instance, calls, emitted, timers } = settings({}, { "runner-telegram-code": () => code, "support-open": () => open, "runner-sync": () => sync });
  await instance.linkTelegram();
  assert.deepEqual(calls, [["runner-telegram-code"], ["support-open", "bot", "123456"]]);
  assert.deepEqual(emitted, [["updated", { account: { signedIn: true } }]], "снимок профиля из ответа уходит наверх");
  assert.equal(instance.telegramMessage, null);
  assert.equal(instance.telegramCode, "123456");
  assert.equal(instance.telegramBusy, false);

  // Повторное нажатие с живым кодом — сначала синхронизация; привязки нет — бот с тем же кодом,
  // нового кода не просится.
  await instance.linkTelegram();
  assert.deepEqual(calls.slice(2), [["runner-sync"], ["support-open", "bot", "123456"]]);

  // Срок вышел (таймер) — код пропал, следующее нажатие берёт новый.
  assert.equal(timers.length, 1);
  assert.ok(timers[0].ms > 590000 && timers[0].ms <= 600000, String(timers[0].ms));
  timers[0].fn();
  assert.equal(instance.telegramCode, null);
  code = Object.assign({}, code, { code: "654321" });
  await instance.linkTelegram();
  assert.deepEqual(calls.slice(4), [["runner-telegram-code"], ["support-open", "bot", "654321"]]);

  // Игрок уже привязался этим кодом (ревью S-7, находка 3): синхронизация это показывает — бот со
  // старым кодом не открывается, код пропадает, снимок уходит наверх.
  sync = { account: { signedIn: true }, telegram: { linked: true, username: "hlsr_fan" } };
  await instance.linkTelegram();
  assert.deepEqual(calls.slice(6), [["runner-sync"]]);
  assert.equal(instance.telegramCode, null);
  assert.deepEqual(emitted[emitted.length - 1], ["updated", sync]);
  sync = undefined;

  // Привязка пришла синхронизацией — код больше не показывается.
  instance.telegramCode = "654321";
  const watcher = component.watch["telegram.linked"];
  (watcher.handler || watcher).call(instance, true, false);
  assert.equal(instance.telegramCode, null);

  // Бот не открылся — та же ошибка, что раньше.
  for (const failure of [{ status: "error", code: "SPONSOR_LINK_INVALID" }, null, new Error("ipc")]) {
    open = failure;
    await instance.linkTelegram();
    assert.equal(instance.telegramMessage, "#L_RUNNER_TELEGRAM_OPEN_ERROR");
    assert.equal(instance.telegramTone, "error");
  }
});

// «Отвязать» — как у Discord: снимает все Telegram профиля (и привязанные
// раньше по чужому SteamID), снимок без привязки уходит наверх.
test("профиль: «Отвязать» у привязанного Telegram — runner-telegram-unlink, снимок наверх; ошибки видны", async () => {
  const markup = require("node:fs").readFileSync(require("node:path").join(__dirname, "../src/renderer/components/Launcher/RunnerSettings.vue"), "utf8");
  assert.ok(/v-else[^>]*@click="unlinkTelegram"[\s\S]*?#L_RUNNER_TELEGRAM_UNLINK/.test(markup), "кнопка «Отвязать» — вместо «Привязать», когда Telegram привязан");
  let answer = { profile: {}, snapshot: { account: { signedIn: true }, telegram: NOT_LINKED } };
  const { instance, calls, emitted } = settings({ telegram: { linked: true, username: "stranger" } }, { "runner-telegram-unlink": () => answer });
  instance.telegramCode = "123456";
  await instance.unlinkTelegram();
  assert.deepEqual(calls, [["runner-telegram-unlink"]]);
  assert.deepEqual(emitted, [["updated", { account: { signedIn: true }, telegram: NOT_LINKED }]]);
  assert.equal(instance.telegramMessage, null);
  assert.equal(instance.telegramCode, null);
  assert.equal(instance.telegramBusy, false);
  for (const [failure, message] of [[{ status: "error", code: "RUNNER_HTTP_429" }, "#L_RUNNER_DISCORD_RATE"], [{ status: "error", code: "RUNNER_NETWORK" }, "#L_RUNNER_NETWORK"], [null, "#L_RUNNER_NETWORK"], [new Error("ipc"), "#L_RUNNER_NETWORK"]]) {
    answer = failure;
    await instance.unlinkTelegram();
    assert.equal(instance.telegramMessage, message, JSON.stringify(failure));
    assert.equal(instance.telegramTone, "error");
    assert.equal(instance.telegramNote, message, "ошибка видна и у привязанного");
  }
});

test("профиль: «Привязать» — код не выдан: 429, сеть, мусор — ошибка в строке, бот не открывается", async () => {
  const markup = require("node:fs").readFileSync(require("node:path").join(__dirname, "../src/renderer/components/Launcher/RunnerSettings.vue"), "utf8");
  assert.ok(markup.includes('$t("#L_RUNNER_TELEGRAM_CODE", telegramCode)'), "код виден в строке Telegram");
  for (const [answer, message] of [
    [{ status: "error", code: "RUNNER_HTTP_429" }, "#L_RUNNER_DISCORD_RATE"],
    [{ status: "error", code: "RUNNER_NETWORK" }, "#L_RUNNER_NETWORK"],
    [{ status: "error", code: "RUNNER_HTTP_404" }, "#L_RUNNER_FORM_ERROR"],
    [null, "#L_RUNNER_NETWORK"],
    [new Error("ipc"), "#L_RUNNER_NETWORK"],
    [{ code: "12345", expiresAt: 1 }, "#L_RUNNER_FORM_ERROR"],
    [{ code: "123456", expiresAt: "скоро" }, "#L_RUNNER_FORM_ERROR"],
  ]) {
    const { instance, calls } = settings({}, { "runner-telegram-code": () => answer, "support-open": { status: "ok" } });
    await instance.linkTelegram();
    assert.deepEqual(calls, [["runner-telegram-code"]], JSON.stringify(answer));
    assert.equal(instance.telegramMessage, message, JSON.stringify(answer));
    assert.equal(instance.telegramTone, "error");
    assert.equal(instance.telegramCode, null);
    assert.equal(instance.telegramBusy, false);
  }
});

test("профиль: «Обновить» — синхронизация профиля без опроса; снимок уходит наверх, привязки ещё нет — подсказка", async () => {
  let answer = { account: { signedIn: true }, telegram: NOT_LINKED, error: null };
  const { instance, calls, emitted } = settings({}, { "runner-sync": () => answer });
  await instance.refreshTelegram();
  assert.deepEqual(calls, [["runner-sync"]]);
  assert.deepEqual(emitted, [["updated", answer]]);
  assert.equal(instance.telegramMessage, "#L_RUNNER_TELEGRAM_NOT_YET");
  assert.equal(instance.telegramTone, "ok");
  assert.equal(instance.telegramBusy, false);

  answer = { account: { signedIn: true }, telegram: { linked: true, username: "hlsr_fan" }, error: null };
  await instance.refreshTelegram();
  assert.equal(instance.telegramMessage, null, "привязка видна в строке — подсказка не нужна");
  assert.equal(emitted.length, 2);

  answer = { account: { signedIn: true }, telegram: NOT_LINKED, error: "RUNNER_NETWORK" };
  await instance.refreshTelegram();
  assert.equal(instance.telegramMessage, "#L_RUNNER_NETWORK");
  assert.equal(instance.telegramTone, "error");

  for (const failure of [{ status: "error", code: "RUNNER_NETWORK" }, null, new Error("ipc")]) {
    answer = failure;
    const before = emitted.length;
    await instance.refreshTelegram();
    assert.equal(instance.telegramMessage, "#L_RUNNER_NETWORK");
    assert.equal(emitted.length, before, "без снимка наверх ничего не уходит");
  }
});

// «Обновить» до привязки в боте → «Привязка пока не найдена»; игрок
// заканчивает в боте; привязку приносит фоновая синхронизация (новый снимок
// → prop telegram). Подсказка «не найдена» рядом с «Telegram: привязан» —
// неправда: она стирается, а пока привязан, не показывается вовсе.
test("профиль: привязка пришла после «не найдена» — подсказка «не найдена» исчезает", async () => {
  const { component, instance } = settings({}, { "runner-sync": { account: { signedIn: true }, telegram: NOT_LINKED, error: null } });
  await instance.refreshTelegram();
  assert.equal(instance.telegramMessage, "#L_RUNNER_TELEGRAM_NOT_YET");
  assert.equal(instance.telegramNote, "#L_RUNNER_TELEGRAM_NOT_YET", "до привязки подсказка видна");

  // Фоновая синхронизация: ProfileModal отдаёт новый telegram.
  instance.telegram = { linked: true, username: "hlsr_fan" };
  assert.equal(instance.telegramNote, null, "привязан — подсказка «не найдена» не рисуется даже до наблюдателя");
  const watcher = component.watch["telegram.linked"];
  (watcher.handler || watcher).call(instance, true, false);
  assert.equal(instance.telegramMessage, null, "наблюдатель стирает подсказку");
  assert.equal(instance.telegramLabel, "#L_RUNNER_TELEGRAM_LINKED_AS|@hlsr_fan");

  // Ошибка сети при привязанном Telegram видна, как прежде; отвязка ничего не стирает.
  instance.telegramTone = "error";
  instance.telegramMessage = "#L_RUNNER_NETWORK";
  assert.equal(instance.telegramNote, "#L_RUNNER_NETWORK");
  instance.telegram = NOT_LINKED;
  (watcher.handler || watcher).call(instance, false, true);
  assert.equal(instance.telegramMessage, "#L_RUNNER_NETWORK");
  const fs = require("node:fs"), path = require("node:path");
  const markup = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/RunnerSettings.vue"), "utf8");
  assert.ok(markup.includes('v-if="telegramNote"') && markup.includes("$t(telegramNote)"), "строка Telegram рисует telegramNote, а не telegramMessage");
});

test("локали: строки подсветки и Telegram есть в ru и en, неиспользуемых нет, тексты по решению координатора", () => {
  const fs = require("node:fs"), path = require("node:path"), vm = require("node:vm");
  const read = (file) => fs.readFileSync(path.join(__dirname, "..", file), "utf8");
  const context = { module: { exports: {} } };
  vm.runInNewContext(read("src/renderer/components/Launcher/runner-locales.js").replace("export default", "module.exports ="), context);
  const { ru, en } = context.module.exports;
  const used = new Set((read("src/renderer/components/Launcher/StylePresetPicker.vue") + read("src/renderer/components/Launcher/RunnerSettings.vue"))
    .match(/#L_RUNNER_(?:STYLE|TELEGRAM)_[A-Z0-9_]+/g));
  for (const key of used) assert.ok(ru[key] && en[key], key);
  for (const key of [...Object.keys(ru), ...Object.keys(en)].filter((k) => /^#L_RUNNER_(STYLE|TELEGRAM)_/.test(k))) assert.ok(used.has(key), `не используется: ${key}`);
  assert.equal(ru["#L_RUNNER_STYLE_GROUP"], "Подсветка ника и рамка");
  assert.equal(en["#L_RUNNER_STYLE_GROUP"], "Nick highlight and frame");
  assert.equal(ru["#L_RUNNER_STYLE_NONE"], "Без подсветки");
  assert.equal(en["#L_RUNNER_STYLE_NONE"], "No highlight");
  assert.equal(ru["#L_RUNNER_STYLE_PERK_REQUIRED"], "Подсветка доступна поддержавшим проект");
  assert.equal(en["#L_RUNNER_STYLE_PERK_REQUIRED"], "Highlight is available to project supporters");
  assert.equal(ru["#L_RUNNER_STYLE_SUPPORT"], "Поддержать проект");
  assert.equal(en["#L_RUNNER_STYLE_SUPPORT"], "Support the project");
  assert.equal(ru["#L_RUNNER_STYLE_SUB_UNTIL"], "Подписка активна до %0%");
  assert.equal(en["#L_RUNNER_STYLE_SUB_UNTIL"], "Subscription active until %0%");
  assert.equal(ru["#L_RUNNER_STYLE_SUB_ENDED"], "Подписка закончилась");
  assert.equal(en["#L_RUNNER_STYLE_SUB_ENDED"], "Subscription ended");
  assert.equal(ru["#L_RUNNER_TELEGRAM_LINKED_AS"], "Telegram: привязан (%0%)");
  assert.equal(en["#L_RUNNER_TELEGRAM_LINKED_AS"], "Telegram: linked (%0%)");
  assert.equal(ru["#L_RUNNER_TELEGRAM_LINKED"], "Telegram: привязан");
  assert.equal(en["#L_RUNNER_TELEGRAM_LINKED"], "Telegram: linked");
  assert.equal(ru["#L_RUNNER_TELEGRAM_NOT_LINKED"], "Telegram: не привязан");
  assert.equal(en["#L_RUNNER_TELEGRAM_NOT_LINKED"], "Telegram: not linked");
  assert.equal(ru["#L_RUNNER_TELEGRAM_LINK"], "Привязать");
  assert.equal(en["#L_RUNNER_TELEGRAM_LINK"], "Link");
  assert.equal(ru["#L_RUNNER_TELEGRAM_REFRESH"], "Обновить");
  assert.equal(en["#L_RUNNER_TELEGRAM_REFRESH"], "Refresh");
  // Строка Telegram — на «ты» (ревью S-7, находка 4): ни «привяжите», ни «попробуйте».
  for (const key of [...Object.keys(ru).filter((k) => k.startsWith("#L_RUNNER_TELEGRAM_")), "#L_RUNNER_DISCORD_RATE"])
    assert.doesNotMatch(ru[key], /[а-яё]+(?:ите|йте)(?![а-яё])/i, key);
  assert.equal(ru["#L_RUNNER_TELEGRAM_LINK_HINT"], "Нажми «Привязать» — подписка и донаты через бота HLSR засчитаются этому профилю, а подсветка выдастся сама.");
});
