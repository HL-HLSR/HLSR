"use strict";

// Как в официальной сборке: эти значения подставляет webpack (src/main/security/official.js).
process.env.HLSR_API_HOST = process.env.HLSR_API_HOST || "api.hlsr.pro";
process.env.HLSR_DISCORD_APP_ID = process.env.HLSR_DISCORD_APP_ID || "100000000000000001";

// Лента «Главной»: клиент в main разбирает ответ сервера строго и
// при сбое оставляет последнюю удачную ленту.

const test = require("node:test");
const assert = require("node:assert/strict");
const NewsClient = require("../src/main/telemetry/news-client");
const layout = require("../src/shared/news-image-layout");

const post = { kind: "post", id: "12", title: "Обновление", body: "Текст\n\nВторой абзац", date: "2026-10-05T10:00:00+00:00" };
const record = {
  kind: "record", id: "z16q06rm", game: "Half-Life 2", gameKey: "hl2", category: "New Engine",
  runners: ["Buster12"], seconds: 2190.285, date: "2025-07-15T22:14:46+00:00",
  url: "https://www.speedrun.com/runs/z16q06rm",
};

function client(responses, now) {
  const calls = [];
  const instance = new NewsClient({
    endpoint: "http://127.0.0.1:1/v1/public/news",
    now: now || (() => 1000),
    request: async (url, options) => {
      calls.push({ url, options });
      const next = responses.shift();
      if (next instanceof Error) throw next;
      return next;
    },
  });
  return { instance, calls };
}

const ok = (items) => ({ status: 200, headers: {}, body: JSON.stringify({ items }) });

test("лента отдаёт посты и рекорды в нормализованном виде", async () => {
  const { instance } = client([ok([post, record])]);
  const answer = await instance.get();
  assert.equal(answer.status, "ok");
  assert.deepEqual(answer.items.map((item) => item.kind), ["post", "record"]);
  assert.equal(answer.items[0].date, "2026-10-05T10:00:00.000Z");
  assert.equal(answer.items[1].id, "z16q06rm");
});

test("полный список рекордов (records) заменяет их долю в items, сабкатегория сохраняется", async () => {
  const sub = (id, subcategory) => Object.assign({}, record, { id, subcategory, url: `https://www.speedrun.com/hl1/runs/${id}` });
  const records = Array.from({ length: 60 }, (_, i) => sub("run" + String(i).padStart(5, "0"), i % 2 ? "Scripted · SS" : "Scriptless · RTA"));
  const body = JSON.stringify({ items: [post, records[0], records[1]], records });
  const { instance } = client([{ status: 200, headers: {}, body }]);
  const items = (await instance.get()).items;
  assert.equal(items.filter((item) => item.kind === "record").length, 60, "больше 40 и без повторов");
  assert.equal(items.filter((item) => item.kind === "post").length, 1);
  assert.equal(items.find((item) => item.id === "run00001").subcategory, "Scripted · SS");
});

test("подтверждённый забег (run) из records проходит проверки рекорда и сохраняет вид", async () => {
  const run = { ...record, kind: "run", id: "ydxk59vz", url: "https://www.speedrun.com/hl2/runs/ydxk59vz" };
  const body = JSON.stringify({ items: [post, record], records: [run, record, { ...run, id: "bad00001", runners: [] }] });
  const { instance } = client([{ status: 200, headers: {}, body }]);
  const items = (await instance.get()).items;
  assert.deepEqual(items.map((item) => item.kind + ":" + item.id), ["post:12", "run:ydxk59vz", "record:z16q06rm"]);
});

test("ссылка рекорда ведёт на страницу забега с игрой", async () => {
  const fresh = Object.assign({}, record, { id: "y24vkp5z", url: "https://www.speedrun.com/hl2/runs/y24vkp5z" });
  const keyless = Object.assign({}, record, { id: "ylw19jnz", gameKey: "", url: "https://www.speedrun.com/runs/ylw19jnz" });
  const { instance } = client([ok([record, fresh, keyless])]);
  const answer = await instance.get();
  assert.deepEqual(answer.items.map((item) => item.url), [
    "https://www.speedrun.com/hl2/runs/z16q06rm",
    "https://www.speedrun.com/hl2/runs/y24vkp5z",
    "https://www.speedrun.com/runs/ylw19jnz",
  ]);
});

test("чужие ссылки, мусор и неизвестные типы отбрасываются", async () => {
  const bad = [
    Object.assign({}, record, { url: "https://evil.example/runs/z16q06rm" }),
    Object.assign({}, record, { url: "javascript:alert(1)" }),
    Object.assign({}, record, { runners: [] }),
    Object.assign({}, record, { seconds: -5 }),
    Object.assign({}, post, { id: "1 OR 1" }),
    Object.assign({}, post, { title: "" }),
    Object.assign({}, post, { date: "вчера" }),
    { kind: "html", body: "<script>" },
    null,
    "строка",
  ];
  const { instance } = client([ok(bad.concat([post]))]);
  const answer = await instance.get();
  assert.deepEqual(answer.items.map((item) => item.id), ["12"]);
});

test("повторный запрос в течение двух минут берётся из памяти, «Обновить» идёт на сервер", async () => {
  let now = 1000;
  const { instance, calls } = client([ok([post]), ok([record])], () => now);
  await instance.get();
  now += 60 * 1000;
  assert.equal((await instance.get()).items[0].kind, "post");
  assert.equal(calls.length, 1);
  assert.equal((await instance.get({ force: true })).items[0].kind, "record");
  assert.equal(calls.length, 2);
});

test("при сбое сервера остаётся последняя удачная лента с пометкой ошибки", async () => {
  const { instance } = client([ok([post]), { status: 503, headers: {}, body: "" }, new Error("ECONNRESET"), { status: 200, headers: {}, body: "не json" }]);
  await instance.get();
  for (let i = 0; i < 3; i++) {
    const answer = await instance.get({ force: true });
    assert.equal(answer.status, "error");
    assert.deepEqual(answer.items.map((item) => item.id), ["12"]);
  }
});

test("без удачной ленты ошибка отдаёт пустой список", async () => {
  const { instance } = client([new Error("offline")]);
  assert.deepEqual(await instance.get(), { status: "error", items: [] });
});

test("одновременные запросы делят один сетевой вызов", async () => {
  const { instance, calls } = client([ok([post])]);
  await Promise.all([instance.get(), instance.get(), instance.get()]);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].options.maxResponseBytes, 2 * 1024 * 1024);
});

test("посты содержат автора, до шести картинок API и только числовые счётчики", () => {
  const url = "https://api.hlsr.pro/v1/public/news/image/" + "a".repeat(64) + ".png";
  const images = Array.from({ length: 10 }, () => ({ url, width: 1600, height: 900 }));
  const result = NewsClient.parseItem({ ...post, author: "Developer", images, reactions: { like: 3, fire: "5", wow: -1, love: 1.5, celebrate: 2, evil: 100 } });
  assert.equal(result.author, "Developer");
  assert.equal(result.images.length, 6);
  assert.deepEqual(result.reactions, { like: 3, love: 0, fire: 0, wow: 0, celebrate: 2 });
  assert.equal(NewsClient.parseItem({ ...post, author: [] }), null);
  assert.equal(NewsClient.parseItem({ ...post, author: "a".repeat(81) }), null);
  assert.equal(NewsClient.parseItem({ ...post, images: {} }), null);
  assert.equal(NewsClient.parseItem({ ...post, reactions: [] }), null);
});

test("картинки с чужим хостом, поддельным путём и неверными размерами отбрасываются", () => {
  const path = "/v1/public/news/image/" + "a".repeat(64) + ".webp";
  const bad = [
    { url: "https://api.hlsr.pro.evil.example" + path }, { url: "https://api.hlsr.pro@evil.example" + path },
    { url: "http://api.hlsr.pro" + path }, { url: "https://api.hlsr.pro:8443" + path },
    { url: "data:image/png;base64,AA==" }, { url: "https://api.hlsr.pro/evil.svg" }, { url: path + "?redirect=evil" },
    { url: path, width: "1600" }, { url: path, width: 9000 }, { url: path, height: 0 },
  ].map(item => ({ width: 1600, height: 900, ...item }));
  const result = NewsClient.parseItem({ ...post, images: bad.slice(0, 6) });
  assert.deepEqual(result.images, []);
  for (const image of bad) assert.deepEqual(NewsClient.parseItem({ ...post, images: [image] }).images, []);
  assert.equal(NewsClient.parseItem({ ...post, images: [{ url: path, width: 1600, height: 900 }] }).images[0].url, "https://api.hlsr.pro" + path);
  const local = NewsClient.parseItem({ ...post, images: [{ url: path, width: 1600, height: 900 }] }, "http://127.0.0.1:18896/v1/public/news");
  assert.match(local.images[0].url, /^http:\/\/127\.0\.0\.1:18896\//);
});

test("область скриншота: верная доходит до ленты, неверная отбрасывается без потери картинки", () => {
  const url = "https://api.hlsr.pro/v1/public/news/image/" + "a".repeat(64) + ".png";
  const image = (crop) => NewsClient.parseItem({ ...post, images: [{ url, width: 1600, height: 900, crop }] }).images[0];
  assert.deepEqual(image({ x: 0.25, y: 0.1, w: 0.5, h: 0.5 }).crop, { x: 0.25, y: 0.1, w: 0.5, h: 0.5 });
  for (const bad of [null, "0,0,1,1", { x: 0.6, y: 0, w: 0.5, h: 0.5 }, { x: 0, y: 0, w: 0.01, h: 0.5 }, { x: -0.1, y: 0, w: 0.5, h: 0.5 }, { x: 0, y: 0, w: "0.5", h: 0.5 }]) {
    const parsed = image(bad);
    assert.equal(parsed.url, url);
    assert.equal(parsed.crop, undefined);
  }
});

test("раскладка плиток: один скриншот по пропорциям области, несколько — 16:9, нечётный первый 2:1", () => {
  const wide = { width: 1600, height: 900 };
  assert.equal(layout.tileAspect(wide, 0, 1), 16 / 9);
  // Узкая область упирается в 4:3, сверхширокая — в 21:9.
  assert.equal(layout.tileAspect({ ...wide, crop: { x: 0, y: 0, w: 0.3, h: 1 } }, 0, 1), 4 / 3);
  assert.equal(layout.tileAspect({ ...wide, crop: { x: 0, y: 0, w: 1, h: 0.2 } }, 0, 1), 21 / 9);
  assert.equal(layout.tileAspect(wide, 0, 3), 2);
  assert.equal(layout.tileAspect(wide, 1, 3), 16 / 9);
  assert.equal(layout.tileAspect(wide, 0, 2), 16 / 9);
  // Без области картинка 16:9 занимает плитку 16:9 целиком.
  assert.deepEqual(layout.tileImageStyle(wide, 16 / 9), { width: "100%", height: "100%", left: "0%", top: "0%" });
  // Правая половина: картинка вдвое шире плитки и сдвинута на её ширину влево.
  assert.deepEqual(layout.tileImageStyle({ ...wide, crop: { x: 0.5, y: 0, w: 0.5, h: 1 } }, 8 / 9), { width: "200%", height: "100%", left: "-100%", top: "0%" });
});

test("профиль автора: аватар только с CDN Steam, ник проходит проверку стиля, плохой профиль не ломает пост", () => {
  const avatar = "https://avatars.steamstatic.com/" + "b".repeat(40) + "_full.jpg";
  const profile = { steamId64: "76561199155902676", personaName: "Kanoyo", avatarUrl: avatar, nick: { style: "developer", css: "color: #fff; position: absolute" } };
  const result = NewsClient.parseItem({ ...post, author: "Kanoyo", authorProfile: profile });
  assert.equal(result.authorProfile.steamId64, "76561199155902676");
  assert.equal(result.authorProfile.avatarUrl, avatar);
  assert.deepEqual(result.authorProfile.nick, { style: "developer", css: "" });
  assert.equal(NewsClient.parseItem({ ...post, authorProfile: { ...profile, avatarUrl: "https://evil.example/a.jpg" } }).authorProfile.avatarUrl, "");
  assert.equal(NewsClient.parseItem({ ...post, authorProfile: { ...profile, avatarUrl: "http://avatars.steamstatic.com/a.jpg" } }).authorProfile.avatarUrl, "");
  for (const bad of [null, "x", { steamId64: "123" }, { ...profile, steamId64: 76561199155902676 }]) {
    const item = NewsClient.parseItem({ ...post, authorProfile: bad });
    assert.ok(item);
    assert.equal(item.authorProfile, null);
  }
  assert.equal(NewsClient.parseItem(post).authorProfile, null);
});

test("профиль автора: значок подписчика — только при subscriber: true", () => {
  const profile = { steamId64: "76561199155902676", personaName: "Kanoyo" };
  const subscriberOf = (extra) => NewsClient.parseItem({ ...post, authorProfile: { ...profile, ...extra } }).authorProfile.subscriber;
  assert.equal(subscriberOf({ subscriber: true }), true);
  assert.equal(subscriberOf({}), false, "без ключа");
  for (const subscriber of ["true", 1, "1", {}, [true], null, false])
    assert.equal(subscriberOf({ subscriber }), false, JSON.stringify(subscriber));
  assert.equal(NewsClient.parseProfileCard({ personaName: "Без SteamID", subscriber: true }).subscriber, true, "карточка /v2 без SteamID");
  assert.equal(NewsClient.parseProfileCard({ personaName: "Без SteamID", subscriber: "true" }).subscriber, false);
});

test("ответ реакции сохраняет свежие счётчики поверх устаревшего CDN в течение двух минут", async () => {
  let now = 1000;
  const { instance } = client([ok([{ ...post, reactions: { like: 0 } }]), ok([{ ...post, reactions: { like: 0 } }]), ok([{ ...post, reactions: { like: 0 } }])], () => now);
  await instance.get();
  instance.updateReactions(post.id, { like: 1 });
  assert.equal((await instance.get()).items[0].reactions.like, 1);
  assert.equal((await instance.get({ force: true })).items[0].reactions.like, 1);
  now += 121000;
  assert.equal((await instance.get()).items[0].reactions.like, 0);
});

const newsIpc = require("../src/main/telemetry/news-ipc");
function ipcFixture(action, confirmed = true) {
  const handlers = new Map(), opened = [], asked = [], updates = [];
  newsIpc.bind({ handle: (name, fn) => handlers.set(name, fn) }, {
    profile: { action }, client: { updateReactions: (...args) => updates.push(args) },
    confirmLink: async (...args) => { asked.push(args); return confirmed; }, openExternal: async url => opened.push(url),
  });
  return { invoke: (name, ...args) => handlers.get(name)({}, ...args), opened, asked, updates };
}

test("реакции IPC передают только вид и пост; ответ и свои реакции нормализуются", async () => {
  const calls = [];
  const ipc = ipcFixture(async (...args) => { calls.push(args); return { postId: "12", reactions: { like: 1 }, mine: ["like", "evil", "like"] }; });
  const reply = await ipc.invoke("public-news-react", "12", "like");
  assert.equal(reply.status, "ok"); assert.deepEqual(reply.mine, ["like"]);
  assert.deepEqual(calls, [["news/react", { postId: "12", reaction: "like" }]]);
  assert.equal(ipc.updates[0][1].like, 1);
  assert.equal((await ipc.invoke("public-news-react", "999 OR 1", "like")).status, "error");
  assert.equal((await ipc.invoke("public-news-react", "12", "evil")).status, "error");
  assert.equal(calls.length, 1);
});

test("IPC сохраняет код отсутствия входа и лимита, чужой ответ поста не меняет кеш", async () => {
  for (const code of ["RUNNER_AUTH_REQUIRED", "RUNNER_HTTP_429"]) {
    const ipc = ipcFixture(async () => { throw Object.assign(new Error(code), { code }); });
    assert.deepEqual(await ipc.invoke("public-news-react", "12", "like"), { status: "error", code });
    assert.equal(ipc.updates.length, 0);
  }
  const ipc = ipcFixture(async () => ({ postId: "13", reactions: { like: 1 }, mine: ["like"] }));
  assert.equal((await ipc.invoke("public-news-react", "12", "like")).status, "error");
  assert.equal(ipc.updates.length, 0);
});

test("ссылки новостей открываются только после подтверждения настоящего HTTPS-хоста", async () => {
  const ipc = ipcFixture(null);
  for (const url of ["javascript:alert(1)", "http://example.com", "https://user@example.com", "https://example.com:8443", "https://example.com\n"]) {
    assert.equal((await ipc.invoke("public-news-open-link", url)).status, "error");
  }
  assert.equal(ipc.asked.length, 0); assert.equal(ipc.opened.length, 0);
  assert.equal((await ipc.invoke("public-news-open-link", "https://example.com/path")).status, "ok");
  assert.equal(ipc.asked[0][0], "example.com"); assert.deepEqual(ipc.opened, ["https://example.com/path"]);
  const cancelled = ipcFixture(null, false);
  assert.equal((await cancelled.invoke("public-news-open-link", "https://example.com")).status, "cancelled");
  assert.equal(cancelled.opened.length, 0);
});

const markdown = require("../src/shared/news-markdown");
test("Markdown новостей экранирует HTML и даёт только перехватываемые HTTPS-ссылки", () => {
  const html = markdown.render('# Заголовок\n\n**Жирный** и *курсив* `код`\n\n- один\n- два\n\n[Сайт](https://example.com) [плохой](javascript:alert)\n\n<img src=x onerror=evil()>\n\n![img](https://evil.example/x.png)\n\n```\n  код\n```');
  assert.match(html, /<h3>Заголовок<\/h3>/); assert.match(html, /<strong>Жирный<\/strong>/); assert.match(html, /<em>курсив<\/em>/);
  assert.match(html, /<ul><li>один<\/li><li>два<\/li><\/ul>/);
  assert.match(html, /data-href="https:\/\/example.com"/); assert.doesNotMatch(html, /data-href="javascript:|<img|<script/);
  assert.match(html, /&lt;img/); assert.match(html, /<pre><code>  код<\/code><\/pre>/);
});

function homeFixture(invoke) {
  const fs = require("node:fs"), vm = require("node:vm");
  const { parse } = require("vue/compiler-sfc");
  const script = parse(fs.readFileSync(require("node:path").join(__dirname, "../src/renderer/components/Launcher/HomePanel.vue"), "utf8")).descriptor.script.content;
  const context = { module: { exports: {} }, window: { hlsrElectron: { ipcRenderer: { invoke } } }, NewsPost: {}, NewsImageViewer: {}, HomePlayers: {}, formatRun() {} };
  vm.runInNewContext(script.replace(/^import .*;\s*$/gm, "").replace("export default", "module.exports ="), context);
  const component = context.module.exports, instance = component.data();
  component.created.call(instance);
  for (const [name, fn] of Object.entries(component.methods)) instance[name] = fn.bind(instance);
  Object.defineProperty(instance, "posts", { get: () => instance.items.filter(p => p.kind === "post") });
  Object.defineProperty(instance, "records", { get: () => component.computed.records.call(instance) });
  Object.defineProperty(instance, "visibleRecords", { get: () => component.computed.visibleRecords.call(instance) });
  instance.profile = { status: "signed-in", account: { steamId64: "76561198000000001" } };
  instance.items = [NewsClient.parseItem({ ...post, reactions: { like: 1 } })];
  return instance;
}

test("во вкладке «Рекорды» и новые подтверждённые забеги; метка мирового рекорда — только у рекорда", () => {
  const home = homeFixture(async () => null);
  home.items = [post, record, { ...record, kind: "run", id: "ydxk59vz" }].map((item) => NewsClient.parseItem(item));
  assert.deepEqual(home.records.map((item) => item.kind), ["record", "run"]);
  const { parse } = require("vue/compiler-sfc");
  const template = parse(require("node:fs").readFileSync(require("node:path").join(__dirname, "../src/renderer/components/Launcher/HomePanel.vue"), "utf8")).descriptor.template.content;
  // Метка и кубок — у мест 1–3: рекорд — первое место, у забега место приходит с сервера.
  assert.match(template, /<span v-if="placeOf\(record\)" class="home-record__sub home-record__sub--wr">/);
  assert.match(template, /placeOf\(record\) === 1 \? \$t\("#L_HOME_WR"\)/);
  const [wr, run] = home.records;
  assert.equal(home.placeOf(wr), 1);
  assert.equal(home.placeOf(run), 0, "свежий забег без места — без метки");
  assert.equal(home.placeOf({ ...run, place: 2 }), 2);
  assert.equal(home.podiumClass({ ...run, place: 3 }).join(" "), "is-podium is-bronze");
  assert.equal(home.podiumClass(run), null);
  const locales = require("../src/shared/news-locales");
  assert.ok(locales.ru["#L_HOME_WR"] && locales.en["#L_HOME_WR"]);
  assert.ok(locales.ru["#L_HOME_PLACE"] && locales.en["#L_HOME_PLACE"]);
});

test("во «Все» — не больше трёх обычных забегов одной таблицы, в фильтре игры — все", () => {
  const home = homeFixture(async () => null);
  const run = (id, extra) => NewsClient.parseItem({ ...record, kind: "run", id, url: `https://www.speedrun.com/hl2/runs/${id}`, ...extra });
  home.items = [
    ...["fun00001", "fun00002", "fun00003", "fun00004", "fun00005"].map((id) => run(id, { category: "Cutsceneless", subcategory: "Fun%" })),
    run("leg00001", { category: "Cutsceneless", subcategory: "Legacy" }),
    NewsClient.parseItem(record),
    run("hl1r0001", { gameKey: "hl1", game: "Half-Life", category: "Cutsceneless", subcategory: "Fun%", url: "https://www.speedrun.com/hl1/runs/hl1r0001" }),
  ];
  assert.deepEqual(home.visibleRecords.map((item) => item.id), ["fun00001", "fun00002", "fun00003", "leg00001", "z16q06rm", "hl1r0001"]);
  home.recordFilter = "hl2";
  assert.equal(home.visibleRecords.length, 7, "в фильтре игры забеги не урезаются");
});

test("реакция отображается до ответа, повтор блокируется, ошибка откатывает счётчик и выбор", async () => {
  let finish, calls = 0;
  const home = homeFixture(() => { calls++; return new Promise(resolve => { finish = resolve; }); });
  const pending = home.react("12", "like");
  assert.equal(home.posts[0].reactions.like, 2); assert.equal(home.mine["12"][0], "like");
  await home.react("12", "fire"); assert.equal(calls, 1);
  finish({ status: "error", code: "RUNNER_HTTP_429" }); await pending;
  assert.equal(home.posts[0].reactions.like, 1); assert.equal(home.mine["12"].length, 0);
  assert.equal(home.notices["12"], "#L_HOME_REACT_LIMIT"); assert.equal(home.pending["12"], undefined);
});

test("выход во время запроса откатывает оптимизм, поздний ответ не возвращает личные реакции", async () => {
  let finish;
  const home = homeFixture(() => new Promise(resolve => { finish = resolve; }));
  const pending = home.react("12", "like");
  home.onProfile({ status: "signed-out" });
  assert.equal(home.posts[0].reactions.like, 1); assert.equal(Object.keys(home.mine).length, 0);
  finish({ status: "ok", reactions: { like: 2 }, mine: ["like"] }); await pending;
  assert.equal(home.posts[0].reactions.like, 1); assert.equal(Object.keys(home.mine).length, 0);
});

test("без входа реакция предлагает профиль и не вызывает изменяющий IPC", async () => {
  let calls = 0;
  const home = homeFixture(async () => { calls++; });
  home.profile = { status: "signed-out" };
  await home.react("12", "like");
  assert.equal(home.notices["12"], "#L_HOME_REACT_SIGN_IN"); assert.equal(calls, 0);
});

test("поздний запрос личных реакций после выхода игнорируется", async () => {
  let finish;
  const home = homeFixture(() => new Promise(resolve => { finish = resolve; }));
  const pending = home.loadMine(); home.onProfile({ status: "signed-out" });
  finish({ status: "ok", mine: { "12": ["like"] } }); await pending;
  assert.equal(Object.keys(home.mine).length, 0);
});

test("старый ответ своих реакций не перебивает уже подтверждённое нажатие", async () => {
  let finishMine, finishReact;
  const home = homeFixture(channel => new Promise(resolve => { if (channel === 'public-news-mine') finishMine = resolve; else finishReact = resolve; }));
  const old = home.loadMine(), click = home.react('12', 'like');
  finishReact({ status: 'ok', reactions: { like: 2 }, mine: ['like'] }); await click;
  finishMine({ status: 'ok', mine: { '12': [] } }); await old;
  assert.equal(home.mine['12'][0], 'like'); assert.equal(home.posts[0].reactions.like, 2);
});

test("лента принимает обе формы ссылки забега, которые отдаёт speedrun.com в weblink", () => {
  const raw = { ...record, url: 'https://www.speedrun.com/hl2/run/z16q06rm' };
  assert.equal(NewsClient.parseItem(raw).url, raw.url);
});

test("расширение Markdown новостей сохраняет формат и ссылки старого гайда", () => {
  const guide = require('../src/shared/guide-markdown');
  assert.equal(guide.render('# Описание\n\n```\n  код\n```'), '<p># Описание</p><pre><code>код</code></pre>');
  const html = guide.render('[Скрипт](#jump) ![Кадр](images/shot.png)', { scriptIds: new Set(['jump']), imageBase: 'https://example.com/guide' });
  assert.match(html, /data-script="jump"/);
  assert.match(html, /src="https:\/\/example.com\/guide\/images\/shot.png"/);
  assert.doesNotMatch(guide.render('[Чужой](#evil)', { scriptIds: new Set(['jump']) }), /data-script/);
});

test("спонсоры: мусор отбрасывается, сумма сортирует, профиль проходит только валидный", async () => {
  const SponsorsClient = require("../src/main/telemetry/sponsors-client");
  const sponsors = [
    { id: "1", name: "Малый", amount: 300, note: "" },
    { id: "2", name: "Крупный", amount: 15000, note: "спасибо", profile: { steamId64: "76561198000000111", personaName: "Big", avatarUrl: "https://evil.example/a.png" } },
    { id: "3", name: "", amount: 100 },
    { id: "4", name: "Дробный", amount: 1.5 },
    { id: "5", name: "Минус", amount: -1 },
    { id: "1 OR 1", name: "Инъекция", amount: 1 },
    { id: "1", name: "Повтор", amount: 99999 },
  ];
  // Сервер на PHP: /v2 даёт 404, список берётся из /v1 (подробнее — sponsors-selftest.js).
  let calls = 0;
  const client = new SponsorsClient({ request: async (url) => {
    if (url.includes("/v2/")) return { status: 404, body: "" };
    calls++;
    return { status: 200, body: JSON.stringify({ sponsors }) };
  } });
  const answer = await client.get();
  assert.equal(answer.status, "ok");
  assert.equal(answer.version, 1);
  assert.deepEqual(answer.sponsors.map((s) => s.name), ["Крупный", "Малый"]);
  assert.equal(answer.sponsors[0].profile.steamId64, "76561198000000111");
  assert.equal(answer.sponsors[0].profile.avatarUrl, "", "аватар с чужого хоста");
  assert.equal(answer.sponsors[1].profile, null);
  await client.get();
  assert.equal(calls, 1, "кеш не сработал");

  const broken = new SponsorsClient({ request: async () => ({ status: 503, body: "" }) });
  assert.deepEqual(await broken.get(), { status: "error", version: 2, links: {}, active: [], donors: [], past: [] });
});

function playersFixture(invoke) {
  const fs = require("node:fs"), vm = require("node:vm");
  const { parse } = require("vue/compiler-sfc");
  const script = parse(fs.readFileSync(require("node:path").join(__dirname, "../src/renderer/components/Launcher/HomePlayers.vue"), "utf8")).descriptor.script.content;
  const context = { module: { exports: {} }, window: { hlsrElectron: { ipcRenderer: { invoke } } }, NickName: {}, AvatarFrame: {},
    launcherData: { shortNames: { "70": "Half-Life" } }, document: { hidden: false }, setInterval, clearInterval };
  vm.runInNewContext(script.replace(/^import .*;\s*$/gm, "").replace("export default", "module.exports ="), context);
  const component = context.module.exports, instance = Object.assign(component.data(), { signedIn: true, active: true, $refs: {} });
  component.created.call(instance);
  for (const [name, fn] of Object.entries(component.methods)) instance[name] = fn.bind(instance);
  for (const [name, fn] of Object.entries(component.computed)) Object.defineProperty(instance, name, { get: fn.bind(instance) });
  instance.$localization = { locales: { en: {}, ru: {} }, lang: 0 };
  instance.$t = (key, ...vars) => [key, ...vars].join(" ");
  instance.$nextTick = (fn) => fn && fn();
  return instance;
}

const player = (id, extra) => Object.assign({ steamId64: id, personaName: "Runner " + id, details: {}, rating: { score: 0 },
  presence: { online: false, gameId: null, launcherId: null }, achievementCount: 1,
  games: { "hl1-won": { totalPlaySeconds: 36000, launchCount: 3 }, "hl2-4260": { totalPlaySeconds: 5400, launchCount: 2 } } }, extra);

test("игроки: часы и запуски по выбранной игре, поздний ответ прежней сортировки не попадает в список", async () => {
  const pending = [];
  const instance = playersFixture((channel, query) => new Promise((resolve) => pending.push({ query, resolve })));
  instance.mode = "top";
  assert.equal(instance.valueOf(player("1")), "#L_PLAYERS_HOURS 11", "от 10 часов — целые");
  assert.equal(instance.unitOf(player("1")), "", "у часов подпись под числом");
  instance.game = "hl1-won";
  assert.equal(instance.valueOf(player("1")), "#L_PLAYERS_HOURS 10");
  assert.equal(instance.presenceText(player("1", { presence: { online: true, gameId: "hl1-won", launcherId: "70" } })), "#L_PRESENCE_PLAYING Half-Life");
  instance.setSort("rating");
  assert.equal(instance.game, "", "игра не сбросилась для сортировки без игр");
  assert.equal(instance.valueOf(player("1", { rating: { score: 3 } })), "+3");
  instance.setSort("achievements");
  assert.deepEqual(pending.map((item) => item.query.sort), ["rating", "achievements"]);
  pending[1].resolve({ status: "ok", profiles: [player("2", { rank: 1, achievementCount: 5 })], more: true });
  await new Promise((resolve) => setImmediate(resolve));
  pending[0].resolve({ status: "ok", profiles: [player("1", { rank: 1 })], more: false });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual([...instance.top.profiles.map((profile) => profile.steamId64)], ["2"]);
  assert.equal(instance.top.more, true);
  instance.loadTop(true);
  assert.equal(pending[2].query.offset, 0, "страница не выровнена по 50");
  pending[2].resolve({ status: "ok", profiles: [player("2"), player("3")], more: false });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual([...instance.top.profiles.map((profile) => profile.steamId64)], ["2", "3"], "повтор с сервера задвоил строку");
});

test("игроки: в сети без колонки часов, значение — только в рейтинге", () => {
  const { parse } = require("vue/compiler-sfc");
  const template = parse(require("node:fs").readFileSync(require("node:path").join(__dirname, "../src/renderer/components/Launcher/HomePlayers.vue"), "utf8")).descriptor.template.content;
  assert.match(template, /<span v-if="mode === 'top'" class="player__value">/);
});

test("игроки: подпись запусков и достижений склоняется по числу", () => {
  const instance = playersFixture(async () => null);
  instance.mode = "top";
  instance.$localization.lang = 1;
  const launches = (count) => instance.unitOf(player("1", { games: { "hl1-won": { totalPlaySeconds: 0, launchCount: count } } }));
  instance.sort = "launches";
  assert.deepEqual([1, 2, 5, 11, 21, 22, 25, 111].map(launches), ["_ONE", "_FEW", "_MANY", "_MANY", "_ONE", "_FEW", "_MANY", "_MANY"]
    .map((form) => "#L_PLAYERS_UNIT_LAUNCHES" + form));
  instance.sort = "achievements";
  assert.equal(instance.unitOf(player("1", { achievementCount: 3 })), "#L_PLAYERS_UNIT_ACHIEVEMENTS_FEW");
  instance.$localization.lang = 0;
  assert.equal(instance.unitOf(player("1", { achievementCount: 1 })), "#L_PLAYERS_UNIT_ACHIEVEMENTS_ONE");
  assert.equal(instance.unitOf(player("1", { achievementCount: 3 })), "#L_PLAYERS_UNIT_ACHIEVEMENTS_MANY");
});

test("игроки: сбой опроса онлайна оставляет прежний список, лимит показывается отдельно", async () => {
  let answer = { status: "ok", total: 2, playing: 1, profiles: [player("1")] };
  const instance = playersFixture(async () => answer);
  await instance.loadOnline();
  assert.equal(instance.online.total, 2);
  answer = { status: "error", code: "RUNNER_HTTP_429" };
  await instance.loadOnline();
  assert.deepEqual([...instance.online.profiles.map((profile) => profile.steamId64)], ["1"]);
  assert.equal(instance.online.failed, true);
  assert.equal(instance.online.limited, true);
});

test("место 1–3 у свежего забега (run) проходит, у рекорда и вне 1–3 — нет", async () => {
  const run = (id, place) => ({ ...record, kind: "run", id, place, url: `https://www.speedrun.com/hl2/runs/${id}` });
  const body = JSON.stringify({ items: [], records: [run("plc00002", 2), run("plc00003", 3), run("plc00004", 4), run("plc0000x", "2"), { ...record, place: 2 }] });
  const { instance } = client([{ status: 200, headers: {}, body }]);
  const places = Object.fromEntries((await instance.get()).items.map((item) => [item.id, item.place]));
  assert.deepEqual(places, { plc00002: 2, plc00003: 3, plc00004: null, plc0000x: null, z16q06rm: null });
});

test("profiles у бегунов: SteamID64 рядом с runners, тот же отбор имён; кривые — null, без связей — нет поля", async () => {
  const run = (id, extra) => ({ ...record, kind: "run", id, url: `https://www.speedrun.com/hl2/runs/${id}`, ...extra });
  const body = JSON.stringify({ items: [], records: [
    run("prf00001", { runners: ["Alpha", "", "Guest"], profiles: ["76561198000000111", "76561198000000112", null] }),
    run("prf00002", { runners: ["Alpha", "Beta"], profiles: ["not-a-steam-id", 76561198000000111] }),
    run("prf00003", { runners: ["Alpha", "Beta"], profiles: ["76561198000000111"] }),
    run("prf00004", { runners: ["Alpha"] }),
  ] });
  const { instance } = client([{ status: 200, headers: {}, body }]);
  const byId = Object.fromEntries((await instance.get()).items.map((item) => [item.id, item]));
  assert.deepEqual(byId.prf00001.runners, ["Alpha", "Guest"]);
  assert.deepEqual(byId.prf00001.profiles, ["76561198000000111", null], "пустое имя отброшено вместе со своим профилем");
  assert.equal(byId.prf00002.profiles, null, "не SteamID64 — нет связи");
  assert.equal(byId.prf00003.profiles, null, "длина не совпала с runners — не верим");
  assert.equal(byId.prf00004.profiles, null);
});
