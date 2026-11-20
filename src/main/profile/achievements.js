"use strict";

const HOUR_SECONDS = 3600;
const LAUNCHES_GOAL = 25;

// Виды достижений:
//   install — есть у всех, кто поставил лаунчер;
//   hours / launches — считаются по статистике игр с сервера, с прогрессом;
//   event — видно всем, открывается событием в лаунчере (unlock);
//   secret — открывается событием, но пока не открыто, видно только замок
//   без названия и описания (в списке и счётчике оно есть). Описание
//   открытого секретного — загадка, а не условие;
//   server — выдаёт только сервер (дописывает id в details.secrets), лаунчер
//   его только показывает.
// tier — редкость: rainbow, gold, purple; без него — обычное.
// secret: true — серверное, показанное как секретное (замок до получения).
// bonus: true — сверх 100 %: в счётчике «X из Y» его нет.
const ACHIEVEMENTS = [
  {
    id: "first-launch",
    title: "#L_ACH_FIRST_TITLE",
    description: "#L_ACH_FIRST_DESC",
    icon: "fas fa-download",
    kind: "install",
  },
  {
    id: "touch-grass",
    title: "#L_ACH_GRASS_TITLE",
    description: "#L_ACH_GRASS_DESC",
    icon: "fas fa-seedling",
    kind: "hours",
    hours: 50,
  },
  { id: "full-set", title: "#L_ACH_FULLSET_TITLE", description: "#L_ACH_FULLSET_DESC", icon: "fas fa-layer-group", kind: "event" },
  { id: "warmup", title: "#L_ACH_WARMUP_TITLE", description: "#L_ACH_WARMUP_DESC", icon: "fas fa-play", kind: "launches", launches: LAUNCHES_GOAL },
  { id: "marathon", title: "#L_ACH_MARATHON_TITLE", description: "#L_ACH_MARATHON_DESC", icon: "fas fa-hourglass-half", kind: "event" },
  { id: "week-streak", title: "#L_ACH_WEEK_TITLE", description: "#L_ACH_WEEK_DESC", icon: "fas fa-calendar-check", kind: "event" },
  { id: "own-binds", title: "#L_ACH_BINDS_TITLE", description: "#L_ACH_BINDS_DESC", icon: "fas fa-keyboard", kind: "event" },
  { id: "stopwatch", title: "#L_ACH_STOPWATCH_TITLE", description: "#L_ACH_STOPWATCH_DESC", icon: "fas fa-stopwatch", kind: "event" },
  { id: "own-style", title: "#L_ACH_STYLE_TITLE", description: "#L_ACH_STYLE_DESC", icon: "fas fa-paintbrush", kind: "event" },
  { id: "community-voice", title: "#L_ACH_VOICE_TITLE", description: "#L_ACH_VOICE_DESC", icon: "fas fa-heart", kind: "event" },
  { id: "scout", title: "#L_ACH_SCOUT_TITLE", description: "#L_ACH_SCOUT_DESC", icon: "fas fa-binoculars", kind: "event" },
  { id: "chronicler", title: "#L_ACH_CHRONICLER_TITLE", description: "#L_ACH_CHRONICLER_DESC", icon: "fas fa-scroll", kind: "event" },
  {
    id: "back-to-past",
    title: "#L_ACH_PAST_TITLE",
    description: "#L_ACH_PAST_DESC",
    icon: "fas fa-history",
    kind: "secret",
  },
  { id: "wake-up", title: "#L_ACH_WAKE_TITLE", description: "#L_ACH_WAKE_DESC", icon: "fas fa-briefcase", kind: "secret" },
  { id: "almost", title: "#L_ACH_ALMOST_TITLE", description: "#L_ACH_ALMOST_DESC", icon: "fas fa-hourglass-end", kind: "secret" },
  { id: "night-shift", title: "#L_ACH_NIGHT_TITLE", description: "#L_ACH_NIGHT_DESC", icon: "fas fa-moon", kind: "secret" },
  { id: "found-self", title: "#L_ACH_SELF_TITLE", description: "#L_ACH_SELF_DESC", icon: "fas fa-user-check", kind: "secret" },
  // 0.4.9 (договор achievements-v2-spec.md). Часы и запуски — те же тексты с числом, что у «Потрогай траву» и «Разминки».
  { id: "double-shift", title: "#L_ACH_DOUBLE_TITLE", description: "#L_ACH_GRASS_DESC", icon: "fas fa-business-time", kind: "hours", hours: 100 },
  { id: "cursed", title: "#L_ACH_CURSED_TITLE", description: "#L_ACH_GRASS_DESC", icon: "fas fa-skull", kind: "hours", hours: 500, tier: "purple" },
  { id: "avid-runner", title: "#L_ACH_AVID_TITLE", description: "#L_ACH_GRASS_DESC", icon: "fas fa-person-running", kind: "hours", hours: 1000, tier: "gold" },
  { id: "mountain-dweller", title: "#L_ACH_MOUNTAIN_TITLE", description: "#L_ACH_WARMUP_DESC", icon: "fas fa-mountain", kind: "launches", launches: 500, tier: "purple" },
  { id: "shut-it-down", title: "#L_ACH_SHUT_TITLE", description: "#L_ACH_WARMUP_DESC", icon: "fas fa-power-off", kind: "launches", launches: 1000, tier: "gold" },
  { id: "around-the-clock", title: "#L_ACH_CLOCK_TITLE", description: "#L_ACH_CLOCK_DESC", icon: "fas fa-clock", kind: "event", tier: "purple" },
  { id: "hard-worker", title: "#L_ACH_WORKER_TITLE", description: "#L_ACH_WORKER_DESC", icon: "fas fa-hammer", kind: "event" },
  { id: "day-after-day", title: "#L_ACH_DAYS_TITLE", description: "#L_ACH_DAYS_DESC", icon: "fas fa-calendar-days", kind: "event", tier: "purple" },
  { id: "keep-the-flame", title: "#L_ACH_FLAME_TITLE", description: "#L_ACH_FLAME_DESC", icon: "fas fa-fire-flame-curved", kind: "event", tier: "gold" },
  { id: "frankenstein", title: "#L_ACH_FRANK_TITLE", description: "#L_ACH_FRANK_DESC", icon: "fas fa-bolt", kind: "event" },
  { id: "hoarder", title: "#L_ACH_HOARDER_TITLE", description: "#L_ACH_HOARDER_DESC", icon: "fas fa-box-archive", kind: "event" },
  // «Монтажёр»: открывает main, когда во вкладке «Демо» готов экспорт клипа (не предпросмотр).
  { id: "video-editor", title: "#L_ACH_EDITOR_TITLE", description: "#L_ACH_EDITOR_DESC", icon: "fas fa-film", kind: "event" },
  { id: "patron", title: "#L_ACH_PATRON_TITLE", description: "#L_ACH_PATRON_DESC", icon: "fas fa-hand-holding-heart", kind: "server", tier: "gold" },
  { id: "king-of-the-hill", title: "#L_ACH_KING_TITLE", description: "#L_ACH_KING_DESC", icon: "fas fa-crown", kind: "server", tier: "rainbow", secret: true, bonus: true },
  { id: "particle-accelerator", title: "#L_ACH_BOOSTER_TITLE", description: "#L_ACH_BOOSTER_DESC", icon: "fas fa-atom", kind: "server", tier: "gold" },
  { id: "respected", title: "#L_ACH_RESPECT_TITLE", description: "#L_ACH_RESPECT_DESC", icon: "fas fa-star", kind: "server" },
  { id: "piano", title: "#L_ACH_PIANO_TITLE", description: "#L_ACH_PIANO_DESC", icon: "fas fa-music", kind: "secret" },
  { id: "meet-the-maker", title: "#L_ACH_MAKER_TITLE", description: "#L_ACH_MAKER_DESC", icon: "fas fa-code", kind: "secret" },
  { id: "impossible", title: "#L_ACH_IMPOSSIBLE_TITLE", description: "#L_ACH_IMPOSSIBLE_DESC", icon: "fas fa-ban", kind: "secret" },
  { id: "good-memory", title: "#L_ACH_MEMORY_TITLE", description: "#L_ACH_MEMORY_DESC", icon: "fas fa-brain", kind: "secret", tier: "gold" },
  { id: "hesoyam", title: "#L_ACH_HESOYAM_TITLE", description: "#L_ACH_HESOYAM_DESC", icon: "fas fa-car", kind: "secret" },
];

const SECRET_IDS = ACHIEVEMENTS.filter((item) => item.kind === "secret" || item.secret).map((item) => item.id);
const BONUS_IDS = ACHIEVEMENTS.filter((item) => item.bonus).map((item) => item.id);
// Всё, что открывается событием: эти id хранятся списком на сервере
// (details.secrets — имя поля осталось с первой пасхалки) и в secrets.json.
const UNLOCK_IDS = ACHIEVEMENTS.filter((item) => item.kind === "event" || item.kind === "secret").map((item) => item.id);
// Их сервер дописывает в details.secrets сам; через unlock их не открыть.
const SERVER_IDS = ACHIEVEMENTS.filter((item) => item.kind === "server").map((item) => item.id);
const STORED_IDS = UNLOCK_IDS.concat(SERVER_IDS);
// Эти открывает сам интерфейс; остальные — только main по своим событиям.
const RENDERER_UNLOCK_IDS = ["back-to-past", "wake-up", "night-shift", "chronicler", "frankenstein", "hoarder", "piano", "good-memory", "hesoyam"];
const TIERS = ["rainbow", "gold", "purple"];
// Сессия одного запуска: «Марафон» — от 3 часов, «24/7» — от суток.
const SESSION_GOALS = [["marathon", 3 * HOUR_SECONDS], ["around-the-clock", 24 * HOUR_SECONDS]];
// Секретное без названия: не открытое (свой и чужой профиль) и открытое у
// того игрока, но не у смотрящего (sealed).
const HIDDEN = { title: "#L_ACH_SECRET_TITLE", description: "#L_ACH_SECRET_DESC", icon: "fas fa-question", tier: null, hidden: true };
const SEALED = Object.assign({}, HIDDEN, { description: "#L_ACH_SEALED_DESC", icon: "fas fa-lock" });

function sessionAchievements(seconds) {
  return SESSION_GOALS.filter(([, goal]) => seconds >= goal).map(([id]) => id);
}

function totalLaunches(games) {
  let total = 0;
  for (const gameId of Object.keys(games || {})) {
    const count = Number(games[gameId] && games[gameId].launchCount);
    if (Number.isFinite(count) && count > 0) total += count;
  }
  return total;
}

function bestGame(games) {
  let best = null;

  for (const gameId of Object.keys(games || {})) {
    const entry = games[gameId];
    const seconds = entry && Number(entry.totalPlaySeconds);

    if (!Number.isFinite(seconds) || seconds <= 0) continue;
    if (!best || seconds > best.seconds) best = { gameId, seconds };
  }

  return best;
}

const plainObject = (value) => !!value && typeof value === "object" && !Array.isArray(value);
const STEAM_ID = /^7656119\d{10}$/;

// Время получения у одного аккаунта: { id: мс или null }. null — получено,
// но когда, неизвестно.
function accountTimes(raw) {
  const out = {};
  for (const item of ACHIEVEMENTS) {
    const value = raw[item.id];
    if (item.kind !== "install" && (value === null || (Number.isSafeInteger(value) && value > 0 && value <= 253402300799999))) out[item.id] = value;
  }
  return out;
}

// achievement-times.json: { steamId64: { id: мс или null } }. Аккаунта нет в
// файле — у него ещё нет записи. Плоский { id: мс } первой сборки с датами —
// времена account (вход из сохранённой сессии); без входа неизвестно чьи.
function readTimes(raw, account) {
  if (!plainObject(raw)) return {};
  const flat = !Object.keys(raw).some((key) => STEAM_ID.test(key));
  const source = flat ? (account ? { [account]: raw } : {}) : raw;
  const out = {};
  for (const key of Object.keys(source)) if (STEAM_ID.test(key) && plainObject(source[key])) out[key] = accountTimes(source[key]);
  return out;
}

// Записывает время тому, что открыто, но ещё не записано. Событийное — только
// в момент своего unlock (fresh), порог (запуски, часы) и выданное сервером —
// когда его впервые увидели. Без записи аккаунта (первый запуск 0.4.9 или
// первый вход на этом ПК) и пришедшее с сервера с другого ПК — null: дата
// обновления или входа была бы неправдой.
function stamp(times, options, fresh, now) {
  const out = Object.assign({}, times);
  for (const item of evaluate(options)) {
    const kind = ACHIEVEMENTS.find((entry) => entry.id === item.id).kind;
    if (!item.unlocked || kind === "install" || item.id in out) continue;
    out[item.id] = item.id === fresh || (times && (kind === "launches" || kind === "hours" || kind === "server")) ? now : null;
  }
  return out;
}

function evaluate(options) {
  const opts = options || {};
  const best = bestGame(opts.games);
  const bestHours = best ? best.seconds / HOUR_SECONDS : 0;

  const unlocked = Array.isArray(opts.unlocked) ? opts.unlocked : [];
  const times = opts.times || {};
  const at = (id) => (times[id] ? new Date(times[id]).toISOString() : null);

  return ACHIEVEMENTS.map((item) => {
    const base = { id: item.id, title: item.title, description: item.description, icon: item.icon, tier: item.tier || null };

    if (item.kind === "install") {
      return Object.assign(base, { unlocked: true, progress: 1, value: null, goal: null, at: opts.firstRunAt || null });
    }

    if (item.kind === "event" || item.kind === "secret" || item.kind === "server") {
      const open = unlocked.indexOf(item.id) !== -1;
      const out = Object.assign(base, { unlocked: open, progress: open ? 1 : 0, value: null, goal: null, at: at(item.id) });
      if (SECRET_IDS.indexOf(item.id) === -1) return out;
      return Object.assign(out, { secret: true }, open ? null : HIDDEN);
    }

    if (item.kind === "launches") {
      const launches = totalLaunches(opts.games);
      return Object.assign(base, {
        unlocked: launches >= item.launches,
        progress: Math.max(0, Math.min(1, launches / item.launches)),
        value: Math.min(launches, item.launches),
        goal: item.launches,
        at: at(item.id),
      });
    }

    const goal = item.hours;

    return Object.assign(base, {
      unlocked: bestHours >= goal,
      progress: goal > 0 ? Math.max(0, Math.min(1, bestHours / goal)) : 0,
      value: Math.floor(bestHours),
      goal,
      gameId: best ? best.gameId : null,
      at: at(item.id),
    });
  });
}

// Секретное, которого смотрящий (known) не открыл, — замок без названия,
// описания, редкости и id: у чужого профиля открытое там (SEALED), у любого —
// неполученное. Вместо id — метка secret-N, чтобы ключи списка были разными.
function mask(list, known) {
  let n = 0;
  return list.map((item) => {
    if (!item.secret || known.indexOf(item.id) !== -1) return item;
    n += 1;
    return Object.assign({}, item, item.hidden ? null : SEALED, { id: "secret-" + n });
  });
}

// Порядок в профиле: полученные по редкости (радужное, золотое, фиолетовое,
// обычные), потом неполученные, в самом низу — секретные под замком.
function order(list) {
  const rank = (item) => {
    if (!item.unlocked) return item.hidden ? 5 : 4;
    const tier = TIERS.indexOf(item.tier);
    return tier === -1 ? 3 : tier;
  };
  return list.slice().sort((a, b) => rank(a) - rank(b));
}

// Список — до mask: у замков вместо id метка, бонус по ней не узнать.
function summarize(list) {
  const items = (list || []).filter((item) => BONUS_IDS.indexOf(item.id) === -1);
  const unlocked = items.filter((item) => item.unlocked).length;

  return {
    unlocked,
    total: items.length,
    percent: items.length ? Math.round((unlocked / items.length) * 100) : 0,
  };
}

module.exports = { ACHIEVEMENTS, SECRET_IDS, BONUS_IDS, UNLOCK_IDS, SERVER_IDS, STORED_IDS, RENDERER_UNLOCK_IDS, evaluate, mask, order, readTimes, stamp, summarize,
  sessionAchievements, bestGame, totalLaunches, HOUR_SECONDS };
