<template>
  <!-- Игроки HLSR: кто сейчас в сети и рейтинг. Списки отдаёт сервер
       (/v2/runner/players), поэтому они есть только у вошедших. -->
  <div class="players">
    <div class="players__head">
      <div class="players__filters" role="radiogroup" :aria-label="$t('#L_PLAYERS_TAB')">
        <button v-for="item in modes" :key="item.id" type="button" role="radio" class="players__chip" :class="{ active: mode === item.id }" :aria-checked="mode === item.id ? 'true' : 'false'" @click="setMode(item.id)">
          {{ $t(item.label) }}<span v-if="item.id === 'online' && online.loaded" class="players__chip-count">{{ online.total }}</span>
        </button>
      </div>
      <div v-if="mode === 'top'" class="players__filters" role="radiogroup" :aria-label="$t('#L_PLAYERS_SORT')">
        <button v-for="item in sorts" :key="item.id" type="button" role="radio" class="players__chip" :class="{ active: sort === item.id }" :aria-checked="sort === item.id ? 'true' : 'false'" @click="setSort(item.id)">
          <i :class="item.icon" aria-hidden="true"></i>{{ $t(item.label) }}
        </button>
      </div>
      <div v-if="mode === 'top' && perGame" class="players__filters" role="radiogroup" :aria-label="$t('#L_PLAYERS_GAME')">
        <button v-for="item in games" :key="item.id || 'all'" type="button" role="radio" class="players__chip" :class="{ active: game === item.id }" :aria-checked="game === item.id ? 'true' : 'false'" @click="setGame(item.id)">
          {{ item.id ? item.name : $t("#L_HOME_ALL_GAMES") }}
        </button>
      </div>
      <p v-if="mode === 'online' && signedIn && online.loaded" class="players__summary">{{ onlineSummary }}</p>
    </div>

    <div ref="list" class="players__list" :class="{ 'fade-top': !edges.top, 'fade-bottom': !edges.bottom }" @scroll.passive="onScroll">
      <div v-if="!signedIn" class="players__empty">
        <p>{{ $t("#L_PLAYERS_SIGN_IN") }}</p>
        <button type="button" class="players__more" @click="$emit('open-profile')">{{ $t("#L_HOME_SIGN_IN") }}</button>
      </div>
      <p v-else-if="current.loading && !current.profiles.length" class="players__empty">{{ $t("#L_HOME_LOADING") }}</p>
      <p v-else-if="current.failed && !current.profiles.length" class="players__empty" role="alert">{{ $t(current.limited ? "#L_PLAYERS_LIMIT" : "#L_PLAYERS_FAILED") }}</p>
      <p v-else-if="!current.profiles.length" class="players__empty">{{ $t(mode === "online" ? "#L_PLAYERS_NOBODY" : "#L_PLAYERS_EMPTY") }}</p>
      <ol v-else class="players__rows">
        <li v-for="profile in current.profiles" :key="profile.steamId64">
          <button type="button" class="player" :class="{ 'is-me': profile.steamId64 === me }" :title="$t('#L_HOME_AUTHOR_OPEN')" @click="$emit('open-profile', profile.steamId64)">
            <span v-if="mode === 'top' && sort !== 'new'" class="player__rank" :class="profile.rank <= 3 ? 'is-place-' + profile.rank : ''">{{ profile.rank }}</span>
            <AvatarFrame :frame="profile.avatarFrame" :width="3" radius="var(--l-radius-sm)" hole="var(--l-surface-solid)">
              <span class="player__avatar" aria-hidden="true">
                <img v-if="avatarOf(profile)" :src="avatarOf(profile)" alt="" @error="broken[profile.steamId64] = true" />
                <b v-else>{{ letterOf(profile) }}</b>
              </span>
            </AvatarFrame>
            <span class="player__text">
              <span class="player__name"><NickName :nick="profile.nick" :subscriber="profile.subscriber === true">{{ profile.personaName || profile.steamId64 }}</NickName></span>
              <small class="player__status" :class="'is-' + presenceState(profile)"><span class="player__dot" aria-hidden="true"></span><span class="player__status-text">{{ presenceText(profile) }}</span></small>
            </span>
            <span v-if="mode === 'top'" class="player__value">
              <strong>{{ valueOf(profile) }}</strong>
              <small v-if="unitOf(profile)">{{ unitOf(profile) }}</small>
            </span>
          </button>
        </li>
      </ol>
      <button v-if="mode === 'top' && top.more && signedIn" type="button" class="players__more" :disabled="top.loading" @click="loadTop(true)">
        {{ $t(top.loading ? "#L_HOME_LOADING" : "#L_PLAYERS_MORE") }}
      </button>
    </div>
  </div>
</template>

<script>
import NickName from "./NickName";
import AvatarFrame from "./AvatarFrame";
import launcherData from "./launcher-data";

const { ipcRenderer } = window.hlsrElectron;

// Список «В сети» обновляется, пока вкладка открыта: сервер считает
// человека в сети 90 с после последней синхронизации лаунчера.
const ONLINE_REFRESH_MS = 30000;
const PAGE = 50;
// Игры, по которым считаются часы на сервере. У трёх сборок HL2 одно
// короткое имя в launcher-data, поэтому подписи свои.
const GAMES = [
  { id: "", name: "" },
  { id: "hl1-won", name: "Half-Life" },
  { id: "hl2-2707", name: "HL2 2707" },
  { id: "hl2-4260", name: "HL2 4260" },
  { id: "hl2-5288", name: "HL2 5288" },
  { id: "ghosting-renewed", name: "Ghosting" },
];
const SORTS = [
  { id: "hours", label: "#L_PLAYERS_SORT_HOURS", icon: "fas fa-clock" },
  { id: "achievements", label: "#L_PLAYERS_SORT_ACHIEVEMENTS", icon: "fas fa-trophy" },
  { id: "launches", label: "#L_PLAYERS_SORT_LAUNCHES", icon: "fas fa-play" },
  { id: "rating", label: "#L_PLAYERS_SORT_RATING", icon: "fas fa-thumbs-up" },
  { id: "new", label: "#L_PLAYERS_SORT_NEW", icon: "fas fa-user-plus" },
];

function list() {
  return { profiles: [], loading: false, failed: false, limited: false, loaded: false };
}

export default {
  name: "HomePlayers",
  components: { NickName, AvatarFrame },
  emits: ["open-profile"],
  props: {
    active: { type: Boolean, default: false },
    signedIn: { type: Boolean, default: false },
    me: { type: String, default: "" },
  },
  data() {
    return {
      modes: [{ id: "online", label: "#L_PLAYERS_ONLINE" }, { id: "top", label: "#L_PLAYERS_TOP" }],
      sorts: SORTS,
      games: GAMES,
      mode: "online",
      sort: "hours",
      game: "",
      online: Object.assign(list(), { total: 0, playing: 0 }),
      top: Object.assign(list(), { more: false }),
      broken: {},
      edges: { top: true, bottom: true },
    };
  },
  computed: {
    current() {
      return this.mode === "online" ? this.online : this.top;
    },
    perGame() {
      return this.sort === "hours" || this.sort === "launches";
    },
    locale() {
      return Object.keys(this.$localization.locales)[this.$localization.lang] === "ru" ? "ru-RU" : "en-GB";
    },
    onlineSummary() {
      return this.$t("#L_PLAYERS_SUMMARY", this.online.total, this.online.playing);
    },
    plurals() {
      return new Intl.PluralRules(this.locale);
    },
  },
  watch: {
    active(value) {
      if (value) this.refresh(false);
      this.schedule();
    },
    signedIn() {
      this.online = Object.assign(list(), { total: 0, playing: 0 });
      this.top = Object.assign(list(), { more: false });
      if (this.active) this.refresh(false);
      this.schedule();
    },
    current() {
      this.$nextTick(this.onScroll);
    },
  },
  created() {
    this.requests = { online: 0, top: 0 };
    this.timer = null;
  },
  mounted() {
    if (this.active) this.refresh(false);
    this.schedule();
    document.addEventListener("visibilitychange", this.schedule);
  },
  beforeUnmount() {
    this.requests.online++; this.requests.top++;
    clearInterval(this.timer);
    document.removeEventListener("visibilitychange", this.schedule);
  },
  methods: {
    // Кнопка «Обновить» в шапке главной и первое открытие вкладки.
    refresh(force) {
      if (!this.signedIn) return;
      if (this.mode === "online") this.loadOnline();
      else if (force || !this.top.loaded) this.loadTop(false);
    },
    schedule() {
      clearInterval(this.timer);
      this.timer = null;
      if (this.active && this.signedIn && this.mode === "online" && !document.hidden) {
        this.timer = setInterval(() => this.loadOnline(), ONLINE_REFRESH_MS);
      }
    },
    setMode(id) {
      if (id === this.mode) return;
      this.mode = id;
      this.resetScroll();
      this.refresh(false);
      this.schedule();
    },
    setSort(id) {
      if (id === this.sort) return;
      this.sort = id;
      if (!this.perGame) this.game = "";
      this.loadTop(false);
    },
    setGame(id) {
      if (id === this.game) return;
      this.game = id;
      this.loadTop(false);
    },
    async loadOnline() {
      const request = ++this.requests.online;
      this.online.loading = true;
      const reply = await ipcRenderer.invoke("runner-players", { view: "online" }).catch(() => null);
      if (request !== this.requests.online) return;
      this.online.loading = false;
      this.online.loaded = true;
      if (reply && reply.status === "ok" && reply.signedIn !== false) {
        Object.assign(this.online, { profiles: reply.profiles, total: reply.total, playing: reply.playing, failed: false, limited: false });
        this.$nextTick(this.onScroll);
      } else {
        // Прежний список остаётся: при сбое опроса он лишь немного устарел.
        this.online.failed = true;
        this.online.limited = !!reply && reply.code === "RUNNER_HTTP_429";
      }
    },
    // more — следующая страница того же рейтинга; иначе рейтинг заново.
    async loadTop(more) {
      const request = ++this.requests.top;
      const offset = more ? this.top.profiles.length : 0;
      if (!more) { this.top.profiles = []; this.top.more = false; this.resetScroll(); }
      this.top.loading = true;
      const query = { view: "top", sort: this.sort, game: this.perGame ? this.game : "", offset: Math.floor(offset / PAGE) * PAGE };
      const reply = await ipcRenderer.invoke("runner-players", query).catch(() => null);
      if (request !== this.requests.top) return;
      this.top.loading = false;
      this.top.loaded = true;
      if (reply && reply.status === "ok" && reply.signedIn !== false) {
        const known = new Set(this.top.profiles.map((profile) => profile.steamId64));
        this.top.profiles = this.top.profiles.concat(reply.profiles.filter((profile) => !known.has(profile.steamId64)));
        this.top.more = reply.more;
        this.top.failed = false;
        this.top.limited = false;
        this.$nextTick(this.onScroll);
      } else {
        this.top.failed = true;
        this.top.limited = !!reply && reply.code === "RUNNER_HTTP_429";
      }
    },
    avatarOf(profile) {
      return (!this.broken[profile.steamId64] && profile.details && profile.details.avatarUrl) || null;
    },
    letterOf(profile) {
      const name = (profile.personaName || "?").trim();
      return name ? name[0].toUpperCase() : "?";
    },
    presenceState(profile) {
      if (profile.presence.gameId) return "playing";
      return profile.presence.online ? "online" : "offline";
    },
    presenceText(profile) {
      const game = profile.presence.launcherId && launcherData.shortNames[profile.presence.launcherId];
      if (game) return this.$t("#L_PRESENCE_PLAYING", game);
      return this.$t(profile.presence.online ? "#L_PRESENCE_ONLINE" : "#L_PRESENCE_OFFLINE");
    },
    // Часы и запуски — по выбранной игре или по всем.
    sumOf(profile, field) {
      const games = Object.values(profile.games || {});
      if (this.mode === "top" && this.perGame && this.game) {
        const one = profile.games && profile.games[this.game];
        return one ? one[field] : 0;
      }
      return games.reduce((sum, game) => sum + game[field], 0);
    },
    valueOf(profile) {
      const sort = this.sort;
      if (sort === "hours") {
        const hours = this.sumOf(profile, "totalPlaySeconds") / 3600;
        const shown = hours >= 10 ? Math.floor(hours) : Math.floor(hours * 10) / 10;
        return this.$t("#L_PLAYERS_HOURS", shown.toLocaleString(this.locale));
      }
      if (sort === "launches") return this.sumOf(profile, "launchCount").toLocaleString(this.locale);
      if (sort === "achievements") return String(profile.achievementCount);
      if (sort === "rating") return (profile.rating.score > 0 ? "+" : "") + profile.rating.score;
      if (!profile.registeredAt) return "—";
      return new Date(profile.registeredAt).toLocaleDateString(this.locale, { day: "numeric", month: "short", year: "numeric" });
    },
    // Подпись под значением. У часов её нет: «ч» стоит рядом с числом.
    unitOf(profile) {
      const sort = this.sort;
      if (sort === "launches") return this.countWord("#L_PLAYERS_UNIT_LAUNCHES", this.sumOf(profile, "launchCount"));
      if (sort === "achievements") return this.countWord("#L_PLAYERS_UNIT_ACHIEVEMENTS", profile.achievementCount);
      if (sort === "rating") return this.$t("#L_PLAYERS_UNIT_RATING");
      if (sort === "new") return this.$t("#L_PLAYERS_UNIT_JOINED");
      return "";
    },
    // Склонение по числу: запуск, запуска, запусков. Числа здесь целые,
    // поэтому форма «other» (дроби в русском, всё кроме 1 в английском) — MANY.
    countWord(key, count) {
      const form = this.plurals.select(count);
      return this.$t(key + (form === "one" ? "_ONE" : form === "few" ? "_FEW" : "_MANY"));
    },
    onScroll() {
      const list = this.$refs.list;
      if (!list) return;
      this.edges = { top: list.scrollTop <= 1, bottom: list.scrollTop + list.clientHeight >= list.scrollHeight - 1 };
    },
    resetScroll() {
      const list = this.$refs.list;
      if (list) list.scrollTo({ top: 0, behavior: "instant" });
    },
  },
};
</script>

<style lang="scss" scoped>
@use "./round-dot" as *;

.players {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 16px;

  &__head {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  &__filters {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  /* Как .home-chip (HomePanel): кнопки категорий speedrun.com. */
  &__chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 14px;
    border: none;
    border-radius: 6px;
    background: var(--l-chip-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text);
    font-family: inherit;
    font-size: 13px;
    font-weight: 700;
    white-space: nowrap;
    cursor: pointer;
    transition: color var(--l-fast), background var(--l-fast), box-shadow var(--l-fast);

    i { font-size: 11px; }

    &:hover { background: var(--l-chip-hover); box-shadow: inset 0 0 0 1px var(--l-border-strong); }

    /* Оттенок акцента поверх тёмной подложки: на стекле и ярком акценте белый текст читается. */
    &.active {
      background: linear-gradient(var(--l-accent-soft), var(--l-accent-soft)), var(--l-chip-bg);
      box-shadow: inset 0 0 0 2px var(--l-accent);
    }

    /* Кольцо внутри чипа: снаружи его срезает край панели. Тёмный зазор
       виден и на заполненном выбранном чипе. */
    &:focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--l-accent), inset 0 0 0 4px var(--l-bg); }
  }

  &__chip-count {
    min-width: 18px;
    padding: 0 5px;
    border-radius: 9px;
    background: rgba(0, 0, 0, 0.22);
    font-size: 11px;
    line-height: 18px;
    text-align: center;
  }

  &__summary {
    margin: 0;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
  }

  &__list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 0 4px 32px 0;
    margin-right: -4px;
    --fade-top: 0px;
    --fade-bottom: 0px;
    mask-image: linear-gradient(to bottom, transparent 0, #000 var(--fade-top), #000 calc(100% - var(--fade-bottom)), transparent 100%);

    &.fade-top { --fade-top: 32px; }
    &.fade-bottom { --fade-bottom: 48px; }

    &::-webkit-scrollbar { width: 6px; }
    &::-webkit-scrollbar-track { background: transparent; }
    &::-webkit-scrollbar-thumb { border-radius: 3px; background: rgba(255, 255, 255, 0.28); }
    &:hover::-webkit-scrollbar-thumb { background: rgba(255, 255, 255, 0.45); }
  }

  &__rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  &__empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
    margin: 0;
    color: var(--l-text-faint);
    font-size: 14px;
    line-height: 20px;

    p { margin: 0; }
  }

  &__more {
    margin-top: 12px;
    height: 32px;
    padding: 0 16px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.06);
    color: var(--l-text);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast);

    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
    &:disabled { color: var(--l-text-faint); cursor: default; }
    &:focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--l-accent); }
  }
}

.player {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  padding: 10px 16px;
  border: none;
  border-radius: var(--l-radius);
  background: var(--l-surface-solid);
  box-shadow: inset 0 0 0 1px var(--l-border);
  color: inherit;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: box-shadow var(--l-fast), background var(--l-fast);

  &:hover {
    box-shadow: inset 0 0 0 1px var(--l-border-strong);
    background: linear-gradient(rgba(255, 255, 255, 0.03), rgba(255, 255, 255, 0.03)), var(--l-surface-solid);
  }

  &.is-me { box-shadow: inset 0 0 0 1px var(--l-accent); }

  /* Рамка внутри строки: снаружи её срезал бы край списка с маской. */
  &:focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--l-accent); }

  &__rank {
    flex: none;
    width: 28px;
    color: var(--l-text-faint);
    font-size: 14px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    text-align: center;

    &.is-place-1 { color: #ffd166; }
    &.is-place-2 { color: #cfd8e3; }
    &.is-place-3 { color: #e0a370; }
  }

  &__avatar {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    overflow: hidden;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.08);
    color: var(--l-text-dim);
    font-size: 15px;

    img { width: 100%; height: 100%; object-fit: cover; }
  }

  &__text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  &__name {
    /* Свечение значка подписчика и оформленного ника выходит за буквы, а
       overflow ради многоточия срезал бы его: поля под свечение съедаются
       отрицательным отступом, раскладка не меняется. */
    margin: -4px -6px;
    padding: 4px 6px;
    overflow: hidden;
    color: var(--l-text);
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  /* Обрезается только текст: overflow на всей строке срезал бы свечение точки */
  &__status {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
    white-space: nowrap;

    &.is-playing { color: #4ad07a; }
    &.is-online { color: var(--l-text-dim); }
  }

  &__status-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  &__dot {
    flex: none;
    width: 7px;
    height: 7px;
    @include round-dot(#8b939e);

    .is-online > & { --dot-color: #4ad07a; }
    .is-playing > & { --dot-color: #4ad07a; @include round-dot-glow(4px, rgba(74, 208, 122, 0.7)); }
  }

  &__value {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;

    strong {
      color: var(--l-text);
      font-size: 15px;
      font-weight: 700;
      line-height: 20px;
      font-variant-numeric: tabular-nums;
    }

    small {
      color: var(--l-text-faint);
      font-size: 11px;
      line-height: 14px;
    }
  }
}
</style>
