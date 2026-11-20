"use strict";

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");
const config = require("../telemetry/config");
const transport = require("../telemetry/transport");
const SecureSession = require("./SecureSession");
const schema = require("./profile-state");
const achievements = require("../profile/achievements");
const launchStreak = require("../profile/launch-streak");
const moderation = require("./moderation");

// Рейтинг игроков: страница и глубина — как у сервера (Hlsr\Profile\Players).
const PLAYERS_SORTS = ["hours", "achievements", "launches", "rating", "new"];
const PLAYERS_PAGE = 50;
const PLAYERS_DEPTH = 500;
const TOKEN = /^hlsr_[a-f0-9]{16}_[A-Za-z0-9_-]{43}$/;

class OnlineProfileService {
  constructor(options) {
    const opts = options || {};
    this.root = opts.userDataDir;
    this.base = opts.apiBase || config.API_BASE;
    this.version = opts.launcherVersion || "0.0.0";
    this.request = opts.request || transport.request;
    this.accountProvider = opts.accountProvider || (() => ({}));
    this.friendProvider = opts.friendProvider || (() => []);
    this.openExternal = opts.openExternal || (() => Promise.resolve());
    this.onChange = opts.onChange || (() => {});
    this.onAuthenticated = opts.onAuthenticated || (() => {});
    this.onConsent = opts.onConsent || (() => {});
    this.onUnlock = opts.onUnlock || (() => {});
    this.onModeration = opts.onModeration || (() => {});
    this.onSupportThanks = opts.onSupportThanks || (() => {});
    // Отпечаток сборки (security/build-identity.js) и уведомление о том, что
    // сервер признал лаунчер изменённым.
    this.buildHeader = opts.buildHeader || (() => Promise.resolve(null));
    this.ticketProvider = opts.ticketProvider || (() => Promise.resolve(null));
    this.onClientModified = opts.onClientModified || (() => {});
    this.now = opts.now || (() => Date.now());
    this.sessionFile = new SecureSession(path.join(this.root, "online-session.json"), opts.safeStorage);
    this.token = null;
    this.profile = null;
    this.connected = false;
    this.pending = null;
    this.lastError = null;
    this.pollTimer = null;
    this.timer = null;
    this.syncInFlight = false;
    this.active = new Map();
    // Файл прежнего переноса часов «до обновления»: перенос убран, файл не нужен.
    try { fs.rmSync(path.join(this.root, "online-history-migration.json"), { force: true }); } catch (error) { /* не мешает входу */ }
    // Достижения, открытые событием (и пасхалки). Хранятся и здесь: так
    // достижение не теряется, если сервер недоступен, и уходит на него при
    // следующей синхронизации.
    this.secretsFile = path.join(this.root, "secrets.json");
    const secrets = this.readJson(this.secretsFile);
    this.secrets = (Array.isArray(secrets) ? secrets : []).filter((id) => achievements.UNLOCK_IDS.indexOf(id) !== -1);
    // Найденные адреса страниц забегов (id → ссылка). Старые ссылки
    // /runs/<id> открываются только через API speedrun.com, поэтому адреса
    // узнаются заранее и запоминаются: нажатие сразу открывает браузер.
    this.runLinksFile = path.join(this.root, "speedrun-links.json");
    const links = this.readJson(this.runLinksFile);
    this.runLinks = new Map(Object.entries(links && typeof links === "object" ? links : {})
      .filter(([id, link]) => /^[a-z0-9]{8}$/i.test(id) && typeof link === "string" && schema.RUN_PAGE.test(link)));
    this.runLinkLoads = new Map();
    // Записи модерации, о которых уже было уведомление (id из журнала сервера).
    this.moderationFile = path.join(this.root, "moderation-seen.json");
    const seen = this.readJson(this.moderationFile);
    this.moderationSeen = (Array.isArray(seen) ? seen : []).filter((id) => Number.isSafeInteger(id) && id > 0);
    // Оплаты, за которые уже показано «Спасибо»: сервер повторяет их в каждой синхронизации 30 дней.
    this.thanksFile = path.join(this.root, "support-thanks-seen.json");
    const thanks = this.readJson(this.thanksFile);
    this.thanksSeen = (Array.isArray(thanks) ? thanks : []).filter((id) => typeof id === "string" && id.length <= 128).slice(-200);
    this.moderation = moderation.normalize(null);
    const saved = this.sessionFile.read();
    if (saved && TOKEN.test(saved.token)) {
      const profile = schema.normalize(saved.profile);
      if (profile) { this.token = saved.token; this.profile = profile; }
    }
    // Когда получено каждое достижение на этом компьютере, по аккаунтам
    // (только для показа, на сервер не уходит). Записи аккаунта ещё нет — до
    // 0.4.9 или до первого входа здесь время не хранилось.
    this.timesFile = path.join(this.root, "achievement-times.json");
    this.times = achievements.readTimes(this.readJson(this.timesFile), this.profile && this.profile.steamId64);
  }

  readJson(file) {
    try {
      if (fs.statSync(file).size > 256 * 1024) return null;
      return JSON.parse(fs.readFileSync(file, "utf8"));
    } catch (error) { return null; }
  }

  writeJson(file, data) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const temp = file + ".tmp";
    fs.writeFileSync(temp, JSON.stringify(data), { mode: 0o600 });
    fs.renameSync(temp, file);
  }

  getAuthToken() { return this.token; }

  snapshot() {
    const out = schema.state(this.profile, this.accountProvider(), this.connected, this.secrets,
      { online: this.connected, gameId: this.playingGame() }, this.profile && this.times[this.profile.steamId64]);
    // Серию входов знает только этот компьютер: у чужих профилей её нет.
    return Object.assign(out, { streak: launchStreak.stats(this.root, this.now()), status: this.pending ? "pending" : this.token ? "signed-in" : "signed-out",
      userCode: this.pending ? this.pending.userCode : null,
      expiresAt: this.pending ? new Date(this.pending.expiresAt).toISOString() : null,
      error: this.lastError,
      ban: this.token && moderation.activeAt(this.moderation.ban, this.now()) ? this.moderation.ban : null,
      mute: this.token && moderation.activeAt(this.moderation.mute, this.now()) ? this.moderation.mute : null });
  }

  // Состояние модерации из ответа сервера; о новых записях — уведомление.
  acceptModeration(raw) {
    this.moderation = moderation.normalize(raw);
    const fresh = moderation.unseen(this.moderation, this.moderationSeen);
    if (!fresh.length) return;
    this.moderationSeen = moderation.remember(this.moderationSeen, fresh);
    try { this.writeJson(this.moderationFile, this.moderationSeen); } catch (error) { /* повторное уведомление не страшно */ }
    for (const notice of fresh) {
      try { this.onModeration(notice); } catch (error) { /* уведомление не должно ронять синхронизацию */ }
    }
  }

  // «Спасибо за поддержку»: о каждой оплате из ответа сервера — одно уведомление.
  acceptSupportThanks(list) {
    const fresh = list.filter((item) => item && typeof item.id === "string" && item.id.length <= 128
      && (item.kind === "subscription" || item.kind === "donation") && !this.thanksSeen.includes(item.id));
    if (!fresh.length) return;
    this.thanksSeen = this.thanksSeen.concat(fresh.map((item) => item.id)).slice(-200);
    try { this.writeJson(this.thanksFile, this.thanksSeen); } catch (error) { /* повторное окно не страшно */ }
    for (const item of fresh) {
      const until = Number.isSafeInteger(item.until) && item.until > 0 && item.until <= 253402300799 ? item.until : null;
      try { this.onSupportThanks({ id: item.id, kind: item.kind, until, styles: item.styles === true }); } catch (error) { /* не роняет синхронизацию */ }
    }
  }

  changed() { this.onChange(this.snapshot()); }

  save() {
    if (this.token && this.profile) {
      try { this.sessionFile.write({ token: this.token, profile: this.profile }); } catch (error) { this.sessionFile.clear(); }
    }
  }

  clearAuth(code) {
    this.token = null;
    this.profile = null;
    this.friendsCache = null;
    this.connected = false;
    this.moderation = moderation.normalize(null);
    this.sessionFile.clear();
    this.lastError = code || null;
    this.changed();
  }

  // Выход из аккаунта и уведомление, когда сервер признал сборку изменённой
  // (по хешу app.asar или по ответу на вызов). Возвращает ошибку для throw.
  clientModified() {
    this.cancelLogin();
    if (this.token) this.clearAuth("RUNNER_CLIENT_MODIFIED");
    else { this.lastError = "RUNNER_CLIENT_MODIFIED"; this.changed(); }
    if (!this.modifiedNoticed) {
      this.modifiedNoticed = true;
      try { this.onClientModified(); } catch (error) { /* уведомление не должно ронять запрос */ }
    }
    return Object.assign(new Error("RUNNER_CLIENT_MODIFIED"), { code: "RUNNER_CLIENT_MODIFIED" });
  }

  refusalCode(response) {
    try { return JSON.parse(response.body).error || null; } catch (error) { return null; }
  }

  // Один запрос с билетом сборки (security/attestation.js). Билета нет у
  // dev-запуска и у версий без ключа — сервер таких не просит.
  async send(endpoint, body, headers, build, forceTicket) {
    const sent = Object.assign({}, headers);
    if (build) {
      let ticket = null;
      try { ticket = await this.ticketProvider(forceTicket); } catch (error) {
        if (error && error.code === "CLIENT_MODIFIED") throw this.clientModified();
      }
      if (ticket) sent["x-hlsr-ticket"] = ticket;
    }
    return this.request(this.base + endpoint, { method: "POST", body: JSON.stringify(body || {}),
      headers: sent, timeoutMs: 10000, maxResponseBytes: 2 * 1024 * 1024, launcherVersion: this.version });
  }

  async call(endpoint, body, authorized) {
    const headers = {};
    if (authorized !== false) {
      if (!this.token) throw Object.assign(new Error("RUNNER_AUTH_REQUIRED"), { code: "RUNNER_AUTH_REQUIRED" });
      headers.authorization = "Bearer " + this.token;
    }
    const requestToken = this.token;
    const build = await this.buildHeader();
    if (build) headers["x-hlsr-build"] = build;
    let response = await this.send(endpoint, body, headers, build, false);
    // Билет сборки истёк или ещё не получен: один повтор с новым билетом.
    if (response.status === 428 && build && this.refusalCode(response) === "ATTEST_REQUIRED") {
      response = await this.send(endpoint, body, headers, build, true);
      if (response.status === 428 && this.refusalCode(response) === "ATTEST_REQUIRED")
        throw Object.assign(new Error("RUNNER_ATTEST_FAILED"), { code: "RUNNER_ATTEST_FAILED" });
    }
    if (authorized !== false && requestToken !== this.token) throw new Error("RUNNER_STALE_RESPONSE");
    if (response.status === 401 && authorized !== false) this.clearAuth("RUNNER_SESSION_EXPIRED");
    // Бан и запрет статуса: сервер присылает их срок и причину.
    if (response.status === 403) {
      let denied = null;
      try { denied = JSON.parse(response.body); } catch (error) { /* обычный отказ */ }
      // Сервер не узнал сборку: app.asar изменён. Вход этого устройства
      // сервер уже отозвал; лаунчер выходит из аккаунта и сообщает об этом.
      if (denied && denied.error === "CLIENT_MODIFIED") throw this.clientModified();
      if (denied && (denied.error === "BANNED" || denied.error === "MUTED")) {
        const key = denied.error === "BANNED" ? "ban" : "mute";
        this.moderation = Object.assign({}, this.moderation, { [key]: moderation.normalize({ [key]: denied[key] })[key] });
        this.changed();
        throw Object.assign(new Error("RUNNER_" + denied.error), { code: "RUNNER_" + denied.error });
      }
    }
    if (response.status === 429) {
      let limited = null;
      try { limited = JSON.parse(response.body); } catch (error) { /* обычный лимит запросов */ }
      if (limited && (limited.error === "RATE_COOLDOWN" || limited.error === "LIKE_LIMIT"))
        throw Object.assign(new Error("RUNNER_" + limited.error), { code: "RUNNER_" + limited.error, cooldown: limited });
    }
    // Версия ещё не в списке сервера (только вышла) или лаунчер слишком
    // старый: онлайн недоступен, но из аккаунта не выкидывает.
    if (response.status === 503 || response.status === 426) {
      let refused = null;
      try { refused = JSON.parse(response.body); } catch (error) { /* обычный сбой */ }
      if (refused && (refused.error === "CLIENT_UNVERIFIED" || refused.error === "CLIENT_OUTDATED"))
        throw Object.assign(new Error("RUNNER_" + refused.error), { code: "RUNNER_" + refused.error });
    }
    // Выбор заготовки подсветки (customize, stylePreset): права нет или
    // заготовку скрыли — у интерфейса для них свои тексты.
    if (response.status === 409) {
      const conflict = this.refusalCode(response);
      // SPEEDRUN_TAKEN: этот speedrun.com уже привязан к другому профилю HLSR.
      if (conflict === "PERK_REQUIRED" || conflict === "PRESET_UNKNOWN" || conflict === "SPEEDRUN_TAKEN")
        throw Object.assign(new Error("RUNNER_" + conflict), { code: "RUNNER_" + conflict });
    }
    if (response.status < 200 || response.status >= 300) throw Object.assign(new Error("RUNNER_HTTP_" + response.status), { code: "RUNNER_HTTP_" + response.status });
    const data = JSON.parse(response.body);
    if (!data || typeof data !== "object") throw new Error("RUNNER_INVALID_RESPONSE");
    return data;
  }

  accept(data) {
    if (data && data.moderation) this.acceptModeration(data.moderation);
    if (data && Array.isArray(data.supportThanks)) this.acceptSupportThanks(data.supportThanks.slice(0, 10));
    if (!data || !data.profile) return;
    const next = schema.normalize(data.profile);
    if (!next || (this.profile && next.steamId64 !== this.profile.steamId64)) throw new Error("RUNNER_IDENTITY_MISMATCH");
    this.profile = next;
    this.connected = true;
    this.lastError = null;
    this.save();
    this.noteTimes(null);
    this.changed();
    if (next.details && next.details.pinnedRun) this.warmRunLinks([next.details.pinnedRun.url]);
  }

  start() {
    if (this.timer) return;
    // Список друзей подтягивается сразу после синхронизации: панель друзей
    // при первом открытии берёт его из памяти, а не ждёт Steam.
    this.sync().then(() => { if (this.token) return this.friends(); }).catch(() => {});
    this.timer = setInterval(() => {
      if (this.token) {
        this.sync().catch(() => {});
        for (const [id, session] of this.active) this.beat(id, session, false).catch(() => {});
      }
    }, 30000);
    if (this.timer.unref) this.timer.unref();
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    this.cancelLogin();
    for (const [id, session] of this.active) this.beat(id, session, true).catch(() => {});
  }

  async beginLogin(consent) {
    this.cancelLogin();
    const started = await this.call("/v1/link/start", {}, false);
    const verify = new URL(started.verifyUrl);
    if (verify.origin !== new URL(this.base).origin || verify.pathname !== "/link" ||
      !/^[A-Z0-9]{4}-[A-Z0-9]{4}$/.test(started.userCode) || !/^[a-f0-9]{64}$/.test(started.deviceCode))
      throw new Error("RUNNER_INVALID_LOGIN_URL");
    const pending = { deviceCode: started.deviceCode, userCode: started.userCode,
      expiresAt: this.now() + Math.min(900, Math.max(30, started.expiresIn || 900)) * 1000, consent: consent === true };
    this.pending = pending;
    this.lastError = null;
    this.changed();
    try {
      await this.openExternal(verify.href);
    } catch (error) {
      this.cancelLogin();
      this.lastError = "RUNNER_BROWSER_FAILED";
      this.changed();
      throw new Error("RUNNER_BROWSER_FAILED", { cause: error });
    }
    this.schedulePoll(pending, Math.max(3, started.interval || 3) * 1000);
    return this.snapshot();
  }

  schedulePoll(pending, delay) {
    if (this.pending !== pending) return;
    this.pollTimer = setTimeout(() => this.poll(pending).catch(() => {}), delay);
    if (this.pollTimer.unref) this.pollTimer.unref();
  }

  async poll(pending) {
    if (this.pending !== pending) return;
    if (this.now() >= pending.expiresAt) { this.cancelLogin(); this.lastError = "RUNNER_LOGIN_EXPIRED"; this.changed(); return; }
    try {
      const result = await this.call("/v1/link/poll", { deviceCode: pending.deviceCode, device: process.platform === "win32" ? "Windows" : "Linux" }, false);
      if (this.pending !== pending) return;
      if (result.state === "ready") {
        if (!TOKEN.test(result.token) || !/^7656119\d{10}$/.test(result.steamId64)) throw new Error("RUNNER_INVALID_LOGIN_RESPONSE");
        this.token = result.token;
        this.profile = schema.normalize(result.profile) || schema.normalize({ steamId64: result.steamId64, personaName: "", games: {}, details: {} });
        if (this.profile.steamId64 !== result.steamId64) throw new Error("RUNNER_IDENTITY_MISMATCH");
        this.save();
        this.cancelLogin();
        await this.sync();
        if (!this.token) return;
        this.onConsent(pending.consent);
        this.onAuthenticated(this.snapshot());
        for (const [id, session] of this.active) this.beat(id, session, false).catch(() => {});
        return;
      }
      if (result.state === "expired") { this.cancelLogin(); this.lastError = "RUNNER_LOGIN_EXPIRED"; this.changed(); return; }
      this.schedulePoll(pending, Math.max(3, result.interval || 3) * 1000);
    } catch (error) {
      if (this.pending === pending) { this.lastError = "RUNNER_NETWORK"; this.changed(); this.schedulePoll(pending, 5000); }
    }
  }

  cancelLogin() {
    if (this.pollTimer) clearTimeout(this.pollTimer);
    this.pollTimer = null;
    this.pending = null;
  }

  async sync() {
    if (!this.token || this.syncInFlight) return this.snapshot();
    this.syncInFlight = true;
    try {
      const local = this.accountProvider();
      const persona = local && this.profile && local.steamId64 === this.profile.steamId64 ? local.personaName : this.profile && this.profile.personaName;
      this.accept(await this.call("/v2/runner/sync", { personaName: persona || "", launcherVersion: this.version }));
      // Достижение не должно ронять синхронизацию, если сервер его ещё не
      // знает: отказ по одному не мешает остальным, обрыв сети — ждёт.
      for (const id of this.unsyncedSecrets()) {
        try { this.accept(await this.call("/v2/runner/secret", { id })); } catch (error) {
          if (!/^RUNNER_HTTP_4/.test(error.code || "")) break;
        }
      }
    } catch (error) {
      this.connected = false;
      this.lastError = this.token ? "RUNNER_NETWORK" : this.lastError;
      this.changed();
    } finally { this.syncInFlight = false; }
    return this.snapshot();
  }

  // Отмечает день для серий (7, 14, 30, 100 дней): при старте лаунчера и при запуске игры.
  noteActiveDay() {
    this.activeDay = launchStreak.dayOf(this.now());
    const days = launchStreak.record(this.root, this.now());
    for (const [id, goal] of launchStreak.GOALS) if (days >= goal) this.achieve(id);
  }

  // По фокусу окна: лаунчер открыт со вчера — новый день засчитывается без
  // перезапуска; в тот же день ничего не делает. До первой отметки при
  // старте (окно ещё грузится) — тоже: поздравление ушло бы в пустое окно.
  noteNewDay() {
    if (this.activeDay && launchStreak.dayOf(this.now()) !== this.activeDay) this.noteActiveDay();
  }

  // Игра, в которую сейчас играют: её видно в профиле рядом с онлайном.
  playingGame() {
    for (const [id, session] of this.active) if (!session.ending) return id;
    return null;
  }

  gameStart(id) {
    this.noteActiveDay();
    if (schema.GAMES.indexOf(id) === -1 || this.active.has(id)) return;
    const session = { token: crypto.randomBytes(32).toString("hex"), sequence: 0, started: false, busy: false, ending: false };
    this.active.set(id, session);
    this.changed();
    this.beat(id, session, false).catch(() => {});
  }

  gameEnd(id) {
    const session = this.active.get(id);
    if (!session) return;
    session.ending = true;
    this.changed();
    this.beat(id, session, true).catch(() => {});
  }

  async beat(id, session, end) {
    if (!this.token) { if (end) this.active.delete(id); return; }
    if (session.busy) { if (end) session.ending = true; return; }
    session.busy = true;
    try {
      if (!session.started) {
        this.accept(await this.call("/v2/runner/session/start", { gameId: id, sessionToken: session.token }));
        session.started = true;
      }
      // Решение «конец или пульс» фиксируется до запроса: если игра закрылась,
      // пока пульс был в пути, сессию закрывает отдельный запрос в finally.
      const ending = end || session.ending;
      if (ending || session.sequence > 0) {
        session.sequence++;
        this.accept(await this.call(ending ? "/v2/runner/session/end" : "/v2/runner/session/heartbeat",
          { sessionToken: session.token, sequence: session.sequence }));
      } else session.sequence++;
      if (ending) this.active.delete(id);
    } finally {
      session.busy = false;
      if (session.ending && this.active.has(id)) {
        this.active.delete(id);
        this.call("/v2/runner/session/end", { sessionToken: session.token, sequence: ++session.sequence }).then((data) => this.accept(data)).catch(() => {});
      }
    }
  }

  async action(action, body) {
    const allowed = ["customize", "delete/challenge", "delete/request", "delete/cancel", "speedrun/link", "speedrun/unlink", "speedrun/bests", "speedrun/pin", "speedrun/unpin", "news/react", "news/mine", "discord/code", "discord/unlink", "telegram/code", "telegram/unlink"];
    if (allowed.indexOf(action) === -1) throw new Error("RUNNER_UNKNOWN_ACTION");
    const result = await this.call("/v2/runner/" + action, body || {});
    this.accept(result);
    if (action === "customize" && this.profile && this.profile.details.country === "KP") this.achieve("impossible");
    if (Array.isArray(result.runs)) this.warmRunLinks(result.runs.map((run) => run && run.url));
    return Object.assign({}, result, { snapshot: this.snapshot() });
  }

  unsyncedSecrets() {
    const known = (this.profile && this.profile.details.secrets) || [];
    return this.secrets.filter((id) => known.indexOf(id) === -1);
  }

  // Время получения: после каждого ответа сервера (новая статистика) и при
  // unlock; fresh — то, что открыто только что.
  noteTimes(fresh) {
    if (!this.profile) return;
    const account = this.profile.steamId64, mine = this.times[account];
    const next = achievements.stamp(mine, { games: this.profile.games,
      unlocked: (this.profile.details.secrets || []).concat(this.secrets) }, fresh, this.now());
    if (mine && Object.keys(next).length === Object.keys(mine).length) return;
    this.times = Object.assign({}, this.times, { [account]: next });
    try { this.writeJson(this.timesFile, this.times); } catch (error) { /* время останется до перезапуска */ }
  }

  // Открывает достижение, которое даётся событием. unlocked: true только в
  // первый раз — тогда onUnlock просит интерфейс показать поздравление.
  // Достижения есть только у профиля: без входа ничего не открывается.
  async unlock(id) {
    if (achievements.UNLOCK_IDS.indexOf(id) === -1) throw new Error("RUNNER_UNKNOWN_SECRET");
    if (!this.token || !this.profile) return { unlocked: false, snapshot: this.snapshot() };
    const known = (this.profile && this.profile.details.secrets) || [];
    const unlocked = this.secrets.indexOf(id) === -1 && known.indexOf(id) === -1;
    if (this.secrets.indexOf(id) === -1) {
      this.secrets.push(id);
      try { this.writeJson(this.secretsFile, this.secrets); } catch (error) { /* останется до перезапуска */ }
      this.noteTimes(unlocked ? id : null);
      this.changed();
    }
    if (unlocked) {
      const item = achievements.ACHIEVEMENTS.find((entry) => entry.id === id);
      try { this.onUnlock({ id, title: item.title, icon: item.icon }); } catch (error) { /* поздравление не обязательно */ }
    }
    if (this.token && this.unsyncedSecrets().indexOf(id) !== -1) {
      try { this.accept(await this.call("/v2/runner/secret", { id })); } catch (error) { /* дойдёт при синхронизации */ }
    }
    return { unlocked, snapshot: this.snapshot() };
  }

  // Из интерфейса — только то, что открывает сам интерфейс.
  async unlockFromRenderer(id) {
    if (achievements.RENDERER_UNLOCK_IDS.indexOf(id) === -1) throw new Error("RUNNER_UNKNOWN_SECRET");
    return this.unlock(id);
  }

  // Для событий main: ошибки не мешают тому, что их вызвало.
  achieve(id) {
    this.unlock(id).catch(() => {});
  }

  // Ссылка на страницу забега по его id: speedrun.com отдаёт её в weblink.
  // Найденная запоминается, одновременные запросы одного id ждут один ответ.
  async speedrunRunLink(id) {
    if (!/^[a-z0-9]{8}$/i.test(id)) throw new Error("RUNNER_INVALID_URL");
    if (this.runLinks.has(id)) return this.runLinks.get(id);
    if (!this.runLinkLoads.has(id)) {
      const load = this.fetchRunLink(id).then((link) => {
        this.runLinks.set(id, link);
        while (this.runLinks.size > 500) this.runLinks.delete(this.runLinks.keys().next().value);
        try { this.writeJson(this.runLinksFile, Object.fromEntries(this.runLinks)); } catch (error) { /* останется в памяти */ }
        return link;
      }).finally(() => this.runLinkLoads.delete(id));
      this.runLinkLoads.set(id, load);
    }
    return this.runLinkLoads.get(id);
  }

  async fetchRunLink(id) {
    const response = await this.request("https://www.speedrun.com/api/v1/runs/" + id, { method: "GET", timeoutMs: 10000,
      maxResponseBytes: 512 * 1024, launcherVersion: this.version });
    if (response.status < 200 || response.status >= 300) throw new Error("RUNNER_HTTP_" + response.status);
    const link = JSON.parse(response.body).data.weblink;
    const match = typeof link === "string" ? schema.RUN_PAGE.exec(link) : null;
    if (!match || match[1] !== id) throw new Error("RUNNER_INVALID_URL");
    return link;
  }

  // Адреса старых ссылок (/runs/<id>) узнаются в фоне по одному, чтобы к
  // нажатию они уже были известны.
  warmRunLinks(urls) {
    const ids = (urls || []).map((url) => typeof url === "string" && /^https:\/\/www\.speedrun\.com\/runs\/([a-z0-9]{8})$/i.exec(url))
      .filter(Boolean).map((match) => match[1]).filter((id) => !this.runLinks.has(id));
    if (!ids.length) return;
    this.runLinkQueue = (this.runLinkQueue || Promise.resolve()).then(async () => {
      for (const id of ids) await this.speedrunRunLink(id).catch(() => {});
    });
  }

  async search(query) {
    if (typeof query !== "string" || query.length > 64) return [];
    const data = await this.call("/v2/runner/search", { query });
    const profiles = (Array.isArray(data.profiles) ? data.profiles : []).map(schema.normalize).filter(Boolean);
    if (this.profile && profiles.some((profile) => profile.steamId64 === this.profile.steamId64)) this.achieve("found-self");
    if (profiles.some((profile) => profile.badges.indexOf("developer") !== -1)) this.achieve("meet-the-maker");
    return profiles;
  }

  // Игроки на главной: кто в сети (view: "online") и рейтинг по sort,
  // для часов и запусков — по одной игре (game). Сервер листает рейтинг
  // по PLAYERS_PAGE до PLAYERS_DEPTH места.
  async players(query) {
    const source = query && typeof query === "object" ? query : {};
    if (!this.token || !this.profile) return { status: "ok", signedIn: false };
    const list = (raw) => (Array.isArray(raw) ? raw : []).slice(0, PLAYERS_PAGE * 2).map(schema.listEntry).filter(Boolean);
    const count = (value) => (Number.isSafeInteger(value) && value >= 0 ? value : 0);
    if (source.view === "online") {
      const online = (await this.call("/v2/runner/players", { view: "online" })).online || {};
      return { status: "ok", signedIn: true, view: "online", total: count(online.total), playing: count(online.playing), profiles: list(online.profiles) };
    }
    const offset = source.offset === undefined ? 0 : source.offset;
    if (PLAYERS_SORTS.indexOf(source.sort) === -1 || (source.game && schema.GAMES.indexOf(source.game) === -1)
      || !Number.isSafeInteger(offset) || offset < 0 || offset >= PLAYERS_DEPTH || offset % PLAYERS_PAGE) throw new Error("RUNNER_INVALID_QUERY");
    const top = (await this.call("/v2/runner/players", { view: "top", sort: source.sort, game: source.game || "", offset })).top || {};
    return { status: "ok", signedIn: true, view: "top", sort: source.sort, game: schema.GAMES.indexOf(top.game) !== -1 ? top.game : null,
      offset, more: top.more === true, profiles: list(top.profiles) };
  }

  async view(id) {
    if (!/^7656119\d{10}$/.test(id)) throw new Error("RUNNER_INVALID_ID");
    const data = await this.call("/v2/runner/profile", { steamId64: id });
    const profile = schema.normalize(data.profile);
    if (profile && profile.details && profile.details.pinnedRun) this.warmRunLinks([profile.details.pinnedRun.url]);
    if (profile && this.profile && profile.steamId64 !== this.profile.steamId64) this.achieve("scout");
    if (!profile) return { missing: true, steamId64: id };
    const mine = ((this.profile && this.profile.details.secrets) || []).concat(this.secrets);
    const out = schema.state(profile, {}, true, null, null, null, mine);
    // Список полученного чужим окну не нужен: замки уже в achievements, а id
    // открыли бы, какие секретные у него есть.
    out.details = Object.assign({}, out.details);
    delete out.details.secrets;
    return Object.assign(out, { vote: schema.vote(data.vote) });
  }

  // Лайк (1), дизлайк (-1) или снять голос (0). Поменять голос можно раз в
  // 4 часа: тогда сервер отвечает 429 RATE_COOLDOWN с текущим голосом. Лайков
  // за 4 часа — не больше пяти разным профилям: 429 LIKE_LIMIT и likesAt,
  // когда освободится следующий.
  async rate(id, value) {
    if (!/^7656119\d{10}$/.test(id) || [1, -1, 0].indexOf(value) === -1) throw new Error("RUNNER_INVALID_ID");
    if (this.profile && this.profile.steamId64 === id) throw Object.assign(new Error("RUNNER_SELF_VOTE"), { code: "RUNNER_SELF_VOTE" });
    try {
      const data = await this.call("/v2/runner/rate", { steamId64: id, value });
      return { status: "ok", rating: schema.rating(data.rating), vote: schema.vote(data.vote) };
    } catch (error) {
      if (error && error.cooldown && error.cooldown.error === "LIKE_LIMIT")
        return { status: "like-limit", likesAt: schema.date(error.cooldown.likesAt), rating: schema.rating(error.cooldown.rating), vote: schema.vote(error.cooldown.vote) };
      if (error && error.cooldown) return { status: "cooldown", rating: schema.rating(error.cooldown.rating), vote: schema.vote(error.cooldown.vote) };
      throw error;
    }
  }

  // Список друзей — запрос к Steam через сервер и проверка, кто из них в
  // HLSR: секунды. Поэтому он держится в памяти: без force отдаётся
  // готовый (cached: true), и панель сразу его показывает, а свежий
  // запрашивает следом. Одновременные запросы ждут один и тот же.
  async friends(options) {
    if (!this.token || !this.profile) return { signedIn: false, friends: [] };
    const cached = this.friendsCache && this.friendsCache.steamId64 === this.profile.steamId64 ? this.friendsCache : null;
    if (cached && !(options && options.force)) return Object.assign({}, cached.data, { cached: true });
    if (!this.friendsLoad) {
      const steamId64 = this.profile.steamId64;
      this.friendsLoad = this.loadFriends().then((data) => {
        if (this.profile && this.profile.steamId64 === steamId64) this.friendsCache = { steamId64, data };
        return data;
      }).finally(() => { this.friendsLoad = null; });
    }
    return this.friendsLoad;
  }

  async loadFriends() {
    const local = this.accountProvider();
    const remote = await this.call("/v2/runner/steam-friends", {});
    const list = Array.isArray(remote.friends) ? remote.friends.slice(0, 500).map((friend) => ({ steamId64: friend.steamId64,
      personaName: friend.personaName, avatarDataUri: friend.avatarUrl || null }))
      : local && local.steamId64 === this.profile.steamId64 ? this.friendProvider().slice(0, 500) : [];
    // Пачки по 50 уходят одновременно, а не по очереди.
    const found = new Map(), batches = [];
    for (let at = 0; at < list.length; at += 50) batches.push(list.slice(at, at + 50).map((friend) => friend.steamId64));
    const responses = await Promise.all(batches.map((friends) => this.call("/v2/runner/friends", { friends })));
    for (const response of responses) {
      for (const profile of response.profiles || []) if (profile && typeof profile.steamId64 === "string") found.set(profile.steamId64, true);
    }
    return { signedIn: true, friends: list.map((friend) => Object.assign({}, friend, { inHlsr: found.has(friend.steamId64) })) };
  }

  async logout() {
    for (const [id, session] of this.active) await this.beat(id, session, true).catch(() => {});
    await this.call("/v2/runner/logout", {}).catch(() => {});
    this.clearAuth();
    this.onConsent(false);
    return this.snapshot();
  }

  // В отличие от logout отказ сервера не прячется: другие устройства остались бы в аккаунте.
  async logoutAll() {
    for (const [id, session] of this.active) await this.beat(id, session, true).catch(() => {});
    await this.call("/v2/runner/logout-all", {});
    this.clearAuth();
    this.onConsent(false);
    return this.snapshot();
  }
}

module.exports = OnlineProfileService;
