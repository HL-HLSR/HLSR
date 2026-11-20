<template>
  <!-- Лента и рекорды — отдельные страницы со своей прокруткой. -->
  <section class="launcher-main home-panel">
    <header class="home-panel__head">
      <h1 class="home-panel__title">{{ $t("#L_HOME_TITLE") }}</h1>
      <button
        type="button"
        class="home-panel__refresh"
        :disabled="loading"
        :aria-label="$t('#L_HOME_REFRESH')"
        :title="$t('#L_HOME_REFRESH')"
        @click="refresh"
      >
        <!-- Своя иконка: дуга с центром в середине viewBox, поэтому при
             повороте она не «гуляет», как несимметричный глиф шрифта. -->
        <svg
          ref="spinner"
          class="home-panel__refresh-icon"
          :class="{ spinning: spinning }"
          viewBox="0 0 24 24"
          aria-hidden="true"
          @animationiteration="onSpinTurn"
        >
          <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3" />
          <path d="M19.8 4.2v4.6h-4.6" />
        </svg>
      </button>
    </header>

    <p v-if="failed && activeTab !== 'players'" class="home-panel__note" role="status">
      <i class="fas fa-exclamation-triangle" aria-hidden="true"></i>
      {{ $t(items.length ? "#L_HOME_STALE" : "#L_HOME_FAILED") }}
    </p>

    <nav ref="tabs" class="home-panel__tabs" role="tablist" :aria-label="$t('#L_HOME_TITLE')">
      <button v-for="tab in tabs" :key="tab.id" :ref="el => setTabRef(tab.id, el)" :id="'home-tab-' + tab.id" type="button" role="tab" class="home-panel__tab" :class="{ 'is-active': activeTab === tab.id }" :aria-selected="activeTab === tab.id ? 'true' : 'false'" :aria-controls="'home-page-' + tab.id" :tabindex="activeTab === tab.id ? 0 : -1" @click="selectTab(tab.id)" @keydown="onTabKey($event, tab.id)">{{ $t(tab.label) }}</button>
      <span v-if="ink" class="home-panel__tab-ink l-ink" :class="'is-' + inkMotion" :style="inkStyle" aria-hidden="true"></span>
    </nav>
    <div class="home-panel__pages">
      <transition :name="'l-slide-' + slideDir">
        <div v-show="activeTab === 'feed'" id="home-page-feed" class="home-panel__page" role="tabpanel" aria-labelledby="home-tab-feed">
          <div ref="posts" class="home-panel__list home-panel__stack" :class="fadeClass('posts')" @scroll.passive="onScroll('posts')">
            <NewsPost v-for="post in posts" :key="post.id" :post="post" :date="formatDate(post.date)" :stamp="formatStamp(post.date)" :mine="mine[post.id] || []" :pending="!!pending[post.id]" :notice="notices[post.id] || ''" @react="react" @image="showImage" @sign-in="$emit('open-profile')" @author="$emit('open-profile', $event)" @link="openLink($event, post.id)" @resize="updateEdges" />
            <p v-if="!posts.length" class="home-panel__empty">{{ $t(loading && !loaded ? '#L_HOME_LOADING' : '#L_HOME_NO_POSTS') }}</p>
          </div>
        </div>
      </transition>
      <transition :name="'l-slide-' + slideDir">
        <div v-show="activeTab === 'records'" id="home-page-records" class="home-panel__page" role="tabpanel" aria-labelledby="home-tab-records">
        <div class="home-panel__records-head">
          <h2 class="home-panel__label">{{ $t("#L_HOME_RECORDS") }}</h2>
          <div v-if="recordGames.length > 1" class="home-panel__filters" role="radiogroup" :aria-label="$t('#L_HOME_RECORDS')">
            <button
              v-for="game in filterOptions"
              :key="game.key"
              type="button"
              role="radio"
              class="home-chip"
              :class="{ active: game.key === recordFilter }"
              :aria-checked="game.key === recordFilter ? 'true' : 'false'"
              :title="game.title"
              @click="setFilter(game.key)"
            >{{ game.label }}</button>
          </div>
        </div>
        <div ref="records" class="home-panel__list" :class="fadeClass('records')" @scroll.passive="onScroll('records')">
          <!-- Смена игры: прежний список уходит в сторону прежней кнопки,
               прокрутка сбрасывается, пока списка нет, новый входит с
               другой стороны, карточки — лесенкой. Новые рекорды при
               обновлении всплывают по одной (home-record). -->
          <transition :name="'home-swap-' + filterDir" mode="out-in" @after-leave="resetRecordsScroll">
            <transition-group :key="recordFilter" name="home-record" tag="div" class="home-panel__stack">
              <button
                v-for="(record, index) in visibleRecords"
                :key="'r' + record.id"
                type="button"
                class="home-record"
                :class="podiumClass(record)"
                :style="{ '--i': Math.min(index, 8) }"
                :title="$t('#L_HOME_OPEN_RUN')"
                @click="open(record.url)"
              >
                <span class="home-record__meta">
                  <span class="home-record__game">{{ recordFilter === "all" ? record.game + " · " : "" }}{{ record.category }}</span>
                  <time :datetime="record.date">{{ formatDate(record.date) }}</time>
                </span>
                <span v-if="placeOf(record) || record.subcategory" class="home-record__subs">
                  <span v-if="placeOf(record)" class="home-record__sub home-record__sub--wr">{{ placeOf(record) === 1 ? $t("#L_HOME_WR") : $t("#L_HOME_PLACE", String(placeOf(record))) }}</span>
                  <span v-for="part in record.subcategory.split(' · ').filter(Boolean)" :key="part" class="home-record__sub">{{ part }}</span>
                </span>
                <span class="home-record__main">
                  <strong class="home-record__time">{{ formatRun(record.seconds) }}</strong>
                  <!-- Бегун с профилем HLSR (привязан speedrun.com) — ссылка на профиль; карточка сама —
                       ссылка на забег speedrun.com, поэтому клик по имени до неё не доходит. -->
                  <span class="home-record__runner"><template v-for="(name, i) in record.runners" :key="i">{{ i ? ", " : "" }}<span v-if="profileOf(record, i)" class="home-record__person" role="link" tabindex="0" :title="$t('#L_HOME_RUNNER_PROFILE', name)" @click.stop="$emit('open-profile', profileOf(record, i))" @keydown.enter.stop.prevent="$emit('open-profile', profileOf(record, i))">{{ name }}</span><template v-else>{{ name }}</template></template></span>
                </span>
                <!-- Места 1–3: кубок с номером справа — золото, серебро, бронза (просьба владельца 14.11). -->
                <span v-if="placeOf(record)" class="home-record__trophy" aria-hidden="true">
                  <i class="fas fa-trophy"></i>
                  <span class="home-record__trophy-rank">{{ placeOf(record) }}</span>
                </span>
              </button>
            </transition-group>
          </transition>
          <p v-if="!records.length" class="home-panel__empty">
            {{ $t(loading && !loaded ? "#L_HOME_LOADING" : "#L_HOME_NO_RECORDS") }}
          </p>
        </div>

        </div>
      </transition>
      <transition :name="'l-slide-' + slideDir">
        <div v-show="activeTab === 'players'" id="home-page-players" class="home-panel__page" role="tabpanel" aria-labelledby="home-tab-players">
          <HomePlayers ref="players" :active="activeTab === 'players'" :signed-in="profile.status === 'signed-in'" :me="(profile.account && profile.account.steamId64) || ''" @open-profile="$emit('open-profile', $event)" />
        </div>
      </transition>
    </div>
    <NewsImageViewer v-if="viewer" :images="viewer.images" :initial="viewer.index" :caption="viewer.title" @close="closeImage" />
  </section>
</template>

<script>
import { formatRun } from "./run-time";
import { inkBox, direction } from "./tab-ink";
import NewsPost from "./NewsPost";
import NewsImageViewer from "./NewsImageViewer";
import HomePlayers from "./HomePlayers";

const { ipcRenderer } = window.hlsrElectron;

// Короткие подписи фильтра: полные названия игр speedrun.com не помещаются
// в колонку рекордов, полное название остаётся во всплывающей подсказке.
const GAME_ORDER = ["hl1", "hl1ce", "hl2", "hl2ce", "hl2hl1"];
const GAME_SHORT = { hl1: "HL", hl1ce: "HL CE", hl2: "HL2", hl2ce: "HL2 CE", hl2hl1: "HL2 + HL1" };
const RUNS_PER_BOARD = 3;

export default {
  name: "HomePanel",
  components: { NewsPost, NewsImageViewer, HomePlayers },
  emits: ["open-profile", "overlay"],
  data() {
    return {
      items: [],
      tabs: [{ id: 'feed', label: '#L_HOME_FEED' }, { id: 'records', label: '#L_HOME_RECORDS_TAB' }, { id: 'players', label: '#L_PLAYERS_TAB' }],
      activeTab: 'feed', slideDir: 'next', ink: null, inkMotion: 'still',
      mine: {}, pending: {}, notices: {}, viewer: null, profile: { status: 'signed-out' },
      loading: false,
      loaded: false,
      failed: false,
      spinning: false,
      recordFilter: "all",
      filterDir: "next",
      edges: { posts: { top: true, bottom: true }, records: { top: true, bottom: true } },
    };
  },
  computed: {
    inkStyle() { return this.ink ? { left: this.ink.left + 'px', right: this.ink.right + 'px' } : {}; },
    posts() {
      return this.items.filter((item) => item.kind === "post");
    },
    // Рекорды и свежие подтверждённые забеги — одним списком по дате.
    records() {
      return this.items.filter((item) => item.kind === "record" || item.kind === "run");
    },
    recordGames() {
      const games = new Map();
      for (const record of this.records) {
        const key = record.gameKey || record.game;
        if (!games.has(key)) games.set(key, record.game);
      }
      return [...games.entries()]
        .map(([key, title]) => ({ key, title, label: GAME_SHORT[key] || title }))
        .sort((a, b) => this.gameRank(a.key) - this.gameRank(b.key));
    },
    filterOptions() {
      return [{ key: "all", title: this.$t("#L_HOME_ALL_GAMES"), label: this.$t("#L_HOME_ALL_GAMES") }, ...this.recordGames];
    },
    visibleRecords() {
      if (this.recordFilter !== "all") return this.records.filter((record) => (record.gameKey || record.game) === this.recordFilter);
      // Во «Все» — не больше RUNS_PER_BOARD свежих обычных забегов одной таблицы, чтобы одна
      // популярная категория не заняла весь верх. Рекорды — все.
      const seen = new Map();
      return this.records.filter((record) => {
        if (record.kind !== "run") return true;
        const board = [record.gameKey || record.game, record.category, record.subcategory].join("\n");
        seen.set(board, (seen.get(board) || 0) + 1);
        return seen.get(board) <= RUNS_PER_BOARD;
      });
    },
  },
  watch: {
    '$localization.lang'() { this.$nextTick(() => this.measureInk('still')); },
    loading(value) {
      if (value) this.spinning = true;
      // Без анимации (настройка «без анимаций») конец оборота не наступит.
      else if (!this.$refs.spinner || getComputedStyle(this.$refs.spinner).animationName === "none") this.spinning = false;
    },
    items() {
      if (this.recordFilter !== "all" && !this.recordGames.some((game) => game.key === this.recordFilter)) {
        this.recordFilter = "all";
      }
      this.$nextTick(this.updateEdges);
    },
    visibleRecords() {
      this.$nextTick(() => this.onScroll("records"));
    },
  },
  created() { this.tabEls = {}; this.authRevision = 0; this.mineRequest = 0; this.gone = false; },
  mounted() {
    ipcRenderer.on('runner-profile-state', this.onProfilePush);
    ipcRenderer.invoke('runner-state').then(this.onProfile).catch(() => {});
    this.load(false);
    this.observer = new ResizeObserver(() => { this.measureInk('still'); this.updateEdges(); });
    this.observer.observe(this.$refs.tabs);
    this.$nextTick(() => this.measureInk('still'));
    window.addEventListener("resize", this.updateEdges);
  },
  beforeUnmount() {
    this.gone = true; this.mineRequest++;
    ipcRenderer.removeListener('runner-profile-state', this.onProfilePush);
    if (this.observer) this.observer.disconnect();
    this.$emit('overlay', false);
    window.removeEventListener("resize", this.updateEdges);
  },
  methods: {
    // SteamID64 профиля HLSR i-го бегуна (news-client: profiles) или null.
    profileOf(record, i) {
      return Array.isArray(record.profiles) ? record.profiles[i] || null : null;
    },
    // Место в таблице: мировой рекорд — 1, у свежего забега — с сервера (1–3), иначе 0.
    placeOf(record) {
      return record.kind === "record" ? 1 : record.place || 0;
    },
    podiumClass(record) {
      const place = this.placeOf(record);
      return place ? ["is-podium", ["", "is-gold", "is-silver", "is-bronze"][place]] : null;
    },
    setTabRef(id, el) { if (el) this.tabEls[id] = el; else delete this.tabEls[id]; },
    measureInk(motion) { const el = this.tabEls[this.activeTab], box = inkBox(this.$refs.tabs, el); if (box) { this.inkMotion = this.ink ? motion : 'still'; this.ink = box; } },
    selectTab(id) { this.slideDir = direction(this.tabs.map(t => t.id), this.activeTab, id); this.activeTab = id; this.$nextTick(() => { this.measureInk(this.slideDir); this.updateEdges(); }); },
    onTabKey(event, id) {
      const ids = this.tabs.map(t => t.id); let next;
      if (event.key === 'ArrowRight') next = ids[(ids.indexOf(id) + 1) % ids.length];
      else if (event.key === 'ArrowLeft') next = ids[(ids.indexOf(id) + ids.length - 1) % ids.length];
      else if (event.key === 'Home') next = ids[0];
      else if (event.key === 'End') next = ids[ids.length - 1];
      if (next) { event.preventDefault(); this.selectTab(next); this.tabEls[next].focus(); }
    },
    // На вкладке игроков «Обновить» обновляет и их список.
    refresh() {
      this.load(true);
      if (this.activeTab === 'players' && this.$refs.players) this.$refs.players.refresh(true);
    },
    onProfilePush(event, state) { this.onProfile(state); },
    onProfile(state) {
      if (this.gone || !state || !state.status) return;
      const identity = s => s.status + ':' + (s.account && s.account.steamId64 || '');
      const changed = identity(state) !== identity(this.profile);
      this.profile = state;
      if (changed) {
        this.authRevision++; this.mineRequest++;
        for (const id of Object.keys(this.pending)) this.setCounts(id, this.pending[id].counts);
        this.mine = {}; this.pending = {}; this.notices = {};
        this.loadMine();
      }
    },
    async loadMine() {
      if (this.profile.status !== 'signed-in' || !this.posts.length) return;
      const revision = this.authRevision, request = ++this.mineRequest;
      const reply = await ipcRenderer.invoke('public-news-mine', this.posts.map(p => p.id)).catch(() => null);
      if (!this.gone && revision === this.authRevision && request === this.mineRequest && reply && reply.status === 'ok') {
        for (const id of Object.keys(reply.mine)) if (!this.pending[id]) this.mine[id] = reply.mine[id];
      }
    },
    setCounts(id, reactions) { this.items = this.items.map(post => post.kind === 'post' && post.id === id ? Object.assign({}, post, { reactions }) : post); },
    async react(id, key) {
      if (this.pending[id]) return;
      if (this.profile.status !== 'signed-in') { this.notices[id] = '#L_HOME_REACT_SIGN_IN'; return; }
      const post = this.posts.find(p => p.id === id); if (!post) return;
      this.mineRequest++;
      const before = { counts: Object.assign({}, post.reactions), mine: [...(this.mine[id] || [])] }, revision = this.authRevision;
      this.pending[id] = before; this.notices[id] = '';
      const selected = before.mine.includes(key);
      this.mine[id] = selected ? before.mine.filter(k => k !== key) : before.mine.concat(key);
      this.setCounts(id, Object.assign({}, before.counts, { [key]: Math.max(0, before.counts[key] + (selected ? -1 : 1)) }));
      const reply = await ipcRenderer.invoke('public-news-react', id, key).catch(() => null);
      if (this.gone || revision !== this.authRevision) return;
      this.mineRequest++;
      if (reply && reply.status === 'ok') { this.setCounts(id, reply.reactions); this.mine[id] = reply.mine; }
      else {
        this.setCounts(id, before.counts); this.mine[id] = before.mine;
        this.notices[id] = reply && reply.code === 'RUNNER_HTTP_429' ? '#L_HOME_REACT_LIMIT' : '#L_HOME_REACT_FAILED';
      }
      delete this.pending[id];
    },
    showImage(post, index) { this.viewer = { images: post.images, title: post.title, index }; this.$emit('overlay', true); },
    closeImage() { this.viewer = null; this.$emit('overlay', false); },
    async openLink(url, id) {
      const reply = await ipcRenderer.invoke('public-news-open-link', url).catch(() => null);
      if (!reply || reply.status === 'error') this.notices[id] = '#L_HOME_LINK_FAILED';
    },
    async load(force) {
      this.loading = true;
      try {
        const answer = await ipcRenderer.invoke("public-news-get", { force });
        this.items = answer && Array.isArray(answer.items) ? answer.items : [];
        this.failed = !answer || answer.status !== "ok";
        this.loadMine();
      } catch (error) {
        this.failed = true;
      } finally {
        this.loading = false;
        this.loaded = true;
      }
    },
    onSpinTurn() {
      // Останавливаемся только на целом обороте: иконка не дёргается назад.
      if (!this.loading) this.spinning = false;
    },
    gameRank(key) {
      const index = GAME_ORDER.indexOf(key);
      return index === -1 ? GAME_ORDER.length : index;
    },
    setFilter(key) {
      if (key === this.recordFilter) return;
      this.filterDir = direction(this.filterOptions.map((game) => game.key), this.recordFilter, key);
      this.recordFilter = key;
    },
    // Без плавной прокрутки списка: иначе после смены игры он ехал бы вверх.
    resetRecordsScroll() {
      const list = this.$refs.records;
      if (list) list.scrollTo({ top: 0, behavior: "instant" });
    },
    onScroll(name) {
      const list = this.$refs[name];
      if (!list) return;
      this.edges[name] = {
        top: list.scrollTop <= 1,
        bottom: list.scrollTop + list.clientHeight >= list.scrollHeight - 1,
      };
    },
    updateEdges() {
      this.onScroll("posts");
      this.onScroll("records");
    },
    fadeClass(name) {
      const edge = this.edges[name];
      return { "fade-top": !edge.top, "fade-bottom": !edge.bottom };
    },
    formatRun,
    formatDate(iso) {
      const locale = Object.keys(this.$localization.locales)[this.$localization.lang] === "ru" ? "ru-RU" : "en-GB";
      return new Date(iso).toLocaleDateString(locale, { day: "numeric", month: "long", year: "numeric" });
    },
    // Внизу поста: дата и время публикации по часам игрока.
    formatStamp(iso) {
      const ru = Object.keys(this.$localization.locales)[this.$localization.lang] === "ru";
      const value = new Date(iso);
      const day = value.toLocaleDateString(ru ? "ru-RU" : "en-GB", { day: "numeric", month: "long", year: "numeric" });
      const time = value.toLocaleTimeString(ru ? "ru-RU" : "en-GB", { hour: "2-digit", minute: "2-digit" });
      return this.$t("#L_HOME_POSTED_AT").replace("{date}", day).replace("{time}", time);
    },
    open(url) {
      ipcRenderer.invoke("runner-open-link", url).catch(() => {});
    },
  },
};
</script>

<style lang="scss" scoped>
.home-panel {
  min-height: 0;
  padding: calc(var(--l-topbar-h) + 32px) var(--l-hero-pad) 0;

  &__head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
    margin-bottom: 16px;
  }

  &__title {
    margin: 0;
    color: #fff;
    font-size: 32px;
    font-weight: 700;
    line-height: 40px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  &__refresh {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-text-dim);
    cursor: pointer;
    transition: color var(--l-fast), background var(--l-fast);

    &:hover:not(:disabled) {
      color: var(--l-text);
      background: rgba(255, 255, 255, 0.06);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }

    &:disabled {
      cursor: default;
    }
  }

  &__refresh-icon {
    display: block;
    width: 18px;
    height: 18px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
    stroke-linejoin: round;
    transform-origin: 50% 50%;

    &.spinning {
      animation: home-spin 0.8s linear infinite;
    }
  }

  &__note {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -16px 0 24px;
    color: var(--l-text-dim);
    font-size: 14px;
    line-height: 20px;

    i {
      color: var(--l-orange);
    }
  }

  &__tabs { position: relative; display: flex; gap: 28px; flex: none; height: 42px; margin-bottom: 20px; border-bottom: 1px solid var(--l-border); }
  &__tab { padding: 0 2px; border: none; background: transparent; color: var(--l-text-dim); font: inherit; font-size: 14px; font-weight: 600; cursor: pointer;
    &:hover, &.is-active { color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: -2px; }
  }
  &__tab-ink { bottom: -1px; height: 2px; border-radius: 2px 2px 0 0; background: var(--l-accent); box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 60%, transparent); }
  &__pages { flex: 1; min-height: 0; display: grid; grid-template: minmax(0, 1fr) / minmax(0, 1fr); overflow: hidden; }
  &__page { grid-area: 1/1; display: flex; flex-direction: column; gap: 16px; min-width: 0; min-height: 0; }
  &__label {
    margin: 0;
    color: var(--l-text-dim);
    font-size: 12px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  &__records-head {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  &__filters {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  /* Своя прокрутка у колонки; края плавно гаснут, только когда за ними
     действительно что-то есть. */
  &__list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
    overflow-y: auto;
    overscroll-behavior: contain;
    scroll-behavior: smooth;
    padding: 0 4px 32px 0;
    margin-right: -4px;
    --fade-top: 0px;
    --fade-bottom: 0px;
    mask-image: linear-gradient(
      to bottom,
      transparent 0,
      #000 var(--fade-top),
      #000 calc(100% - var(--fade-bottom)),
      transparent 100%
    );
    transition: --fade-top var(--l-fast), --fade-bottom var(--l-fast);

    &.fade-top {
      --fade-top: 32px;
    }

    &.fade-bottom {
      --fade-bottom: 48px;
    }

    &::-webkit-scrollbar {
      width: 6px;
    }

    &::-webkit-scrollbar-track {
      background: transparent;
    }

    &::-webkit-scrollbar-thumb {
      border-radius: 3px;
      background: rgba(255, 255, 255, 0.28);
    }

    &:hover::-webkit-scrollbar-thumb {
      background: rgba(255, 255, 255, 0.45);
    }
  }

  &__stack > * { flex: none; }

  &__stack {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  &__empty {
    margin: 0;
    color: var(--l-text-faint);
    font-size: 14px;
    line-height: 20px;
  }
}

/* Плавные края маски требуют зарегистрированных свойств. */
@property --fade-top {
  syntax: "<length>";
  inherits: false;
  initial-value: 0px;
}

@property --fade-bottom {
  syntax: "<length>";
  inherits: false;
  initial-value: 0px;
}

/* Кнопки выбора — как категории на speedrun.com: тёмный прямоугольник, жирный белый текст. */
.home-chip {
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
  line-height: 32px;
  white-space: nowrap;
  cursor: pointer;
  transition: color var(--l-fast), background var(--l-fast), box-shadow var(--l-fast);

  &:hover {
    background: var(--l-chip-hover);
    box-shadow: inset 0 0 0 1px var(--l-border-strong);
  }

  /* Оттенок акцента поверх тёмной подложки: на стекле и ярком акценте белый текст читается. */
  &.active {
    background: linear-gradient(var(--l-accent-soft), var(--l-accent-soft)), var(--l-chip-bg);
    box-shadow: inset 0 0 0 2px var(--l-accent);
  }

  &:focus-visible {
    outline: 2px solid var(--l-accent);
    outline-offset: 2px;
  }
}

.home-record {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
  padding: 16px 20px;
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

  &:focus-visible {
    outline: 2px solid var(--l-accent);
    outline-offset: 2px;
  }

  &__meta {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;

    time {
      flex: none;
    }
  }

  &__game {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__main {
    display: flex;
    align-items: baseline;
    gap: 12px;
    min-width: 0;
  }

  &__subs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  &__sub {
    padding: 1px 7px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 11px;
    font-weight: 600;
    line-height: 16px;
    white-space: nowrap;
  }

  &__sub--wr {
    color: var(--l-accent);
  }

  &__time {
    flex: none;
    color: var(--l-accent);
    font-size: 20px;
    font-weight: 700;
    line-height: 24px;
    font-variant-numeric: tabular-nums;
  }

  &__runner {
    min-width: 0;
    overflow: hidden;
    color: var(--l-text);
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__trophy {
    display: none;
  }

  // Имя бегуна с профилем HLSR: подчёркивание при наведении и фокусе.
  &__person {
    border-radius: 3px;
    cursor: pointer;
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 3px;
    transition: color var(--l-fast), text-decoration-color var(--l-fast);

    &:hover,
    &:focus-visible {
      color: var(--l-accent);
      text-decoration-color: currentColor;
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }
  }
}

/* Места 1–3 (золото, серебро, бронза): цвет не зависит от темы — это цвет места.
   Сзади пульсирует свечение (::after), по карточке раз в несколько секунд
   проходит блик (::before); анимируются только opacity и transform. */
.home-record.is-podium {
  --pod-rgb: 245, 197, 66;
  --pod: #f5c542;
  --pod-light: #ffe08a;
  --pod-deep: #b8860b;
  --pod-ink: #4a3200;
  position: relative;
  isolation: isolate;
  overflow: hidden;
  background:
    radial-gradient(140% 160% at 100% 50%, rgba(var(--pod-rgb), 0.2), transparent 55%),
    linear-gradient(90deg, rgba(var(--pod-rgb), 0.07), rgba(var(--pod-rgb), 0) 45%),
    var(--l-surface-solid);
  box-shadow: inset 0 0 0 1px rgba(var(--pod-rgb), 0.5), 0 0 22px rgba(var(--pod-rgb), 0.14);

  &.is-silver {
    --pod-rgb: 200, 212, 226;
    --pod: #d3dce7;
    --pod-light: #f4f7fb;
    --pod-deep: #7d8a99;
    --pod-ink: #262e38;
  }

  &.is-bronze {
    --pod-rgb: 214, 135, 64;
    --pod: #d98a45;
    --pod-light: #f6c08e;
    --pod-deep: #8a4a1a;
    --pod-ink: #3a1c05;
  }

  &:hover {
    background:
      radial-gradient(140% 160% at 100% 50%, rgba(var(--pod-rgb), 0.26), transparent 55%),
      linear-gradient(90deg, rgba(var(--pod-rgb), 0.09), rgba(var(--pod-rgb), 0) 45%),
      var(--l-surface-solid);
    box-shadow: inset 0 0 0 1px rgba(var(--pod-rgb), 0.75), 0 0 26px rgba(var(--pod-rgb), 0.22);
  }

  &::before,
  &::after {
    content: "";
    position: absolute;
    pointer-events: none;
    z-index: -1;
  }

  &::before {
    top: 0;
    bottom: 0;
    left: 0;
    width: 40%;
    background: linear-gradient(105deg, transparent 20%, rgba(var(--pod-rgb), 0.16) 50%, transparent 80%);
    transform: translateX(-120%);
    animation: home-podium-shine 4.5s cubic-bezier(0.45, 0, 0.25, 1) infinite;
  }

  &::after {
    inset: 0;
    background: radial-gradient(60% 120% at 92% 50%, rgba(var(--pod-rgb), 0.22), transparent 70%);
    animation: home-podium-glow 2.8s ease-in-out infinite alternate;
  }

  // Справа внизу — кубок: время и имя не заходят под него, дата сверху остаётся у края.
  .home-record__main {
    padding-right: 64px;
  }

  .home-record__time {
    color: var(--pod);
    text-shadow: 0 0 14px rgba(var(--pod-rgb), 0.35);
  }

  .home-record__sub--wr {
    background: linear-gradient(90deg, rgba(var(--pod-rgb), 0.22), rgba(var(--pod-rgb), 0.1));
    box-shadow: inset 0 0 0 1px rgba(var(--pod-rgb), 0.55);
    color: var(--pod-light);
  }

  .home-record__trophy {
    position: absolute;
    right: 20px;
    bottom: 12px;
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    animation: home-podium-trophy 2.8s ease-in-out infinite alternate;

    i {
      grid-area: 1 / 1;
      font-size: 40px;
      line-height: 1;
      background: linear-gradient(160deg, var(--pod-light) 10%, var(--pod) 45%, var(--pod-deep) 95%);
      -webkit-background-clip: text;
      background-clip: text;
      color: transparent;
      filter: drop-shadow(0 0 8px rgba(var(--pod-rgb), 0.45)) drop-shadow(0 2px 2px rgba(0, 0, 0, 0.45));
    }
  }

  // Номер места в чаше кубка; position — у кубка filter, без этого он рисуется поверх цифры.
  .home-record__trophy-rank {
    position: relative;
    grid-area: 1 / 1;
    margin-top: -9px;
    color: var(--pod-ink);
    font-size: 14px;
    font-weight: 800;
    line-height: 1;
  }
}

@keyframes home-podium-shine {
  0%,
  55% {
    transform: translateX(-120%);
  }
  100% {
    transform: translateX(320%);
  }
}

@keyframes home-podium-glow {
  from {
    opacity: 0.45;
  }
  to {
    opacity: 1;
  }
}

@keyframes home-podium-trophy {
  from {
    transform: scale(1);
  }
  to {
    transform: scale(1.06);
  }
}

// Без анимаций (настройка лаунчера или системная): цвет места остаётся, движение — нет.
.no-anim .home-record.is-podium::before,
.no-anim .home-record.is-podium::after,
.no-anim .home-record.is-podium .home-record__trophy {
  animation: none;
}

.no-anim .home-record.is-podium::before {
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .home-record.is-podium::before,
  .home-record.is-podium::after,
  .home-record.is-podium .home-record__trophy {
    animation: none;
  }

  .home-record.is-podium::before {
    opacity: 0;
  }
}

.home-record-enter-active {
  transition: opacity var(--l-dur) var(--l-ease), transform var(--l-dur) var(--l-ease);
}

.home-record-enter-from {
  opacity: 0;
  transform: translateY(var(--l-rise));
}

.home-swap-next-leave-active,
.home-swap-prev-leave-active {
  transition: opacity var(--l-fast), transform var(--l-fast);
  pointer-events: none;
}

.home-swap-next-leave-to {
  opacity: 0;
  transform: translateX(-24px);
}

.home-swap-prev-leave-to {
  opacity: 0;
  transform: translateX(24px);
}

.home-swap-next-enter-active .home-record,
.home-swap-prev-enter-active .home-record {
  animation: home-record-in var(--l-dur) var(--l-ease) both;
  animation-delay: calc(var(--i, 0) * 28ms);
}

.home-swap-prev-enter-active .home-record {
  animation-name: home-record-in-prev;
}

/* Длительность входа — до конца последней задержанной карточки. */
.home-swap-next-enter-active,
.home-swap-prev-enter-active {
  transition: opacity calc(var(--l-dur) + 224ms) linear;
}

@keyframes home-record-in {
  from {
    opacity: 0;
    transform: translateX(24px);
  }
}

@keyframes home-record-in-prev {
  from {
    opacity: 0;
    transform: translateX(-24px);
  }
}

@keyframes home-spin {
  to {
    transform: rotate(360deg);
  }
}

.no-anim .home-panel__refresh-icon.spinning {
  animation: none;
}

.no-anim .home-panel__list {
  scroll-behavior: auto;
}

.no-anim .home-record-enter-active,
.no-anim .home-swap-next-leave-active,
.no-anim .home-swap-prev-leave-active,
.no-anim .home-swap-next-enter-active,
.no-anim .home-swap-prev-enter-active {
  transition: none;
}

.no-anim .home-swap-next-enter-active .home-record,
.no-anim .home-swap-prev-enter-active .home-record {
  animation: none;
}
</style>
