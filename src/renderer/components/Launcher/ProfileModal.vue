<template>
  <div class="profile" @mousedown.self="$emit('close')">
    <div
      class="profile__window"
      :class="{ 'is-guest': guestWindow, 'is-subscriber': subscriber && !guestWindow, 'is-complete': complete && !guestWindow }"
      role="dialog"
      aria-modal="true"
      :aria-label="windowLabel"
      ref="window"
    >
      <!-- Бегущая рамка (neon.scss), обе темы: голубая у подписчика, золотая
           у того, кто собрал все достижения (100 %). -->
      <span v-if="(subscriber || complete) && !guestWindow" class="neon" aria-hidden="true"></span>
      <!-- Подтверждение входа всплывает поверх окна, а не встаёт в поток:
           иначе через 4 секунды содержимое профиля прыгало вверх. -->
      <transition name="runner-welcome"><div v-if="loginSuccess" class="runner-welcome" role="status"><svg viewBox="0 0 32 32" aria-hidden="true"><circle cx="16" cy="16" r="14"/><path d="M9 16l5 5 9-11"/></svg>{{ $t('#L_RUNNER_SUCCESS') }}</div></transition>

      <button
        type="button"
        class="profile__close"
        :title="$t('#L_CLOSE')"
        :aria-label="$t('#L_CLOSE')"
        ref="closeButton"
        @click="$emit('close')"
      >
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>

      <!-- Чужой профиль: пока он грузится или если не загрузился, окно
           показывает это. Без входа чужие профили не открываются: вместо
           них вход через Steam, после него профиль загрузится сам. -->
      <div v-if="targetSteamId && !guestView && (viewLoading || viewFailed)" class="profile__guest runner-signin profile__pending" role="status">
        <i class="fab fa-steam runner-signin__steam" aria-hidden="true"></i>
        <h2>{{ $t(viewFailed ? '#L_PROFILE_VIEW_FAILED' : '#L_PROFILE_VIEW_LOADING') }}</h2>
        <p v-if="viewFailed">{{ $t('#L_PROFILE_VIEW_FAILED_HINT') }}</p>
        <i v-else class="fas fa-spinner fa-spin profile__spinner" aria-hidden="true"></i>
        <div v-if="viewFailed" class="runner-signin__actions">
          <button type="button" class="runner-signin__button" @click="load">{{ $t('#L_PROFILE_VIEW_RETRY') }}</button>
        </div>
      </div>
      <div v-else-if="missing" class="profile__guest runner-signin">
        <i class="fab fa-steam runner-signin__steam" aria-hidden="true"></i>
        <h2>{{ targetSteamId }}</h2>
        <p>{{ $t('#L_RUNNER_MISSING') }}</p>
      </div>
      <div v-else-if="!account.signedIn && (!targetSteamId || guestView)" class="profile__guest runner-signin">
        <i class="fab fa-steam runner-signin__steam" aria-hidden="true"></i>
        <h2>{{ $t('#L_RUNNER_TITLE') }}</h2>
        <p>{{ $t(guestView ? '#L_RUNNER_VIEW_GUEST' : '#L_RUNNER_LOGIN_TEXT') }}</p>
        <template v-if="loginStatus === 'pending'">
          <p>{{ $t('#L_RUNNER_PENDING') }}</p>
          <strong class="runner-signin__code">{{ loginCode }}</strong>
          <div class="runner-signin__actions">
            <button type="button" class="runner-signin__button" @click="cancelLogin">{{ $t('#L_RUNNER_CANCEL') }}</button>
            <button type="button" class="runner-signin__privacy" @click="openPrivacy">{{ $t('#L_RUNNER_PRIVACY') }}</button>
          </div>
        </template>
        <template v-else>
          <label class="runner-signin__consent">
            <input v-model="consentAccepted" type="checkbox" />
            <span class="runner-signin__box" aria-hidden="true"><i class="fas fa-check"></i></span>
            <span>
              {{ $t('#L_WELCOME_CONSENT_A') }}
              <button type="button" class="runner-signin__privacy" @click.prevent="openPrivacy">{{ $t('#L_WELCOME_PRIVACY') }}</button>
              {{ $t('#L_WELCOME_CONSENT_B') }}
              <button type="button" class="runner-signin__privacy" @click.prevent="openTerms">{{ $t('#L_WELCOME_TERMS') }}</button>
            </span>
          </label>
          <div class="runner-signin__actions">
            <button type="button" class="runner-signin__button is-primary" :disabled="!consentAccepted" @click="signIn"><i class="fab fa-steam" aria-hidden="true"></i> {{ $t('#L_RUNNER_LOGIN') }}</button>
          </div>
          <p class="runner-signin__local">{{ $t('#L_RUNNER_LOCAL_NOTE') }}</p>
        </template>
        <p v-if="loginError" role="alert">{{ $t(LOGIN_ERRORS[loginError] || '#L_RUNNER_NETWORK') }}</p>
      </div>

      <template v-else>
        <!-- Шапка-баннер: кто это. Над всеми вкладками, поэтому ник,
             значки и статус видны и в достижениях, и в настройках. -->
        <header class="profile__hero">
          <div class="profile__banner" :class="{ 'is-developer': isDeveloper }" aria-hidden="true"></div>

          <div class="profile__identity">
            <AvatarFrame :frame="avatarFrame" :width="6" radius="20px" class="profile__avatar-frame">
            <div class="profile__avatar">
              <img
                v-if="account.avatarDataUri"
                :src="account.avatarDataUri"
                :alt="$t('#L_PROFILE_AVATAR_ALT')"
              />
              <i v-else class="fab fa-steam" aria-hidden="true"></i>

              <!-- Точка состояния подписана словами: цвет не единственный
                   признак, иначе состояние теряется при дальтонизме -->
              <span
                class="profile__dot"
                :class="'is-' + presenceState"
                role="img"
                :aria-label="presenceText"
                :title="presenceText"
              ></span>
            </div>
            </AvatarFrame>

            <div class="profile__who">
              <h2 class="profile__name" :title="displayName"><NickName :nick="nick" :subscriber="subscriber" :supporterSince="supporterSince">{{ displayName }}</NickName></h2>

              <!-- Значки — строкой иконок под ником, как в Discord. -->
              <ul v-if="badges.length" class="profile__badges" :aria-label="$t('#L_PROFILE_BADGES')">
                <li
                  v-for="badge in badgeList"
                  :key="badge.id"
                  class="profile__badge-chip"
                  :class="['is-' + badge.id, { 'is-glow': badge.glow, 'is-anniversary': badge.anniversary }]"
                  :style="badge.glow ? { '--glow': badge.glow } : null"
                  :title="badge.label"
                  :aria-label="badge.label"
                >
                  <span v-if="badge.image && badge.themed" class="profile__badge-mask" :style="{ '--badge-icon': 'url(' + badge.image + ')' }" aria-hidden="true"></span>
                  <img v-else-if="badge.image" :src="badge.image" alt="" aria-hidden="true" />
                  <i v-else :class="badge.icon" aria-hidden="true"></i>
                </li>
              </ul>

              <p v-if="shownStatus || shownPronouns || details.country" class="runner-status"><span v-if="shownStatus">{{ shownStatus }}</span><small v-if="shownPronouns || details.country">{{ [shownPronouns, countryName(details.country)].filter(Boolean).join(' · ') }}</small></p>

              <div class="profile__meta">
                <!-- В сети или нет — видно по точке на аватаре, здесь только игра -->
                <span v-if="playingGame" class="profile__since">
                  <i class="fas fa-gamepad" aria-hidden="true"></i>
                  {{ presenceText }}
                </span>
                <span class="profile__since">
                  <i class="fas fa-calendar-day" aria-hidden="true"></i>
                  {{ sinceText }}
                </span>
                <!-- Рейтинг: на чужом профиле лайк и дизлайк, на своём — счёт.
                     Подсказка после голоса всплывает под ним и строку не сдвигает. -->
                <span v-if="account.signedIn && !missing && !guestView" class="profile-rating-anchor">
                <span
                  class="profile-rating"
                  :class="{ 'is-static': !canVote, 'is-negative': rating.score < 0, 'is-positive': rating.score > 0 }"
                  role="group"
                  :aria-label="$t('#L_RATING')"
                  :title="$t('#L_RATING_HINT', String(rating.likes), String(rating.dislikes))"
                >
                  <button
                    v-if="canVote"
                    type="button"
                    class="profile-rating__btn is-like"
                    :class="{ 'is-on': vote.value === 1 }"
                    :aria-pressed="vote.value === 1 ? 'true' : 'false'"
                    :aria-label="$t(vote.value === 1 ? '#L_RATING_UNLIKE' : '#L_RATING_LIKE')"
                    :title="voteTitle(1)"
                    :disabled="voting"
                    @click="castVote(1)"
                  >
                    <i class="fas fa-thumbs-up" aria-hidden="true"></i>
                  </button>
                  <span v-else class="profile-rating__icon" aria-hidden="true"></span>
                  <span class="profile-rating__score">{{ ratingText }}</span>
                  <button
                    v-if="canVote"
                    type="button"
                    class="profile-rating__btn is-dislike"
                    :class="{ 'is-on': vote.value === -1 }"
                    :aria-pressed="vote.value === -1 ? 'true' : 'false'"
                    :aria-label="$t(vote.value === -1 ? '#L_RATING_UNDISLIKE' : '#L_RATING_DISLIKE')"
                    :title="voteTitle(-1)"
                    :disabled="voting"
                    @click="castVote(-1)"
                  >
                    <i class="fas fa-thumbs-down" aria-hidden="true"></i>
                  </button>
                </span>
                <span v-if="voteNote" class="profile-rating__note" role="status">{{ voteNote }}</span>
                </span>
                <span v-if="rating.dubious && !missing" class="profile-rating__dubious" :title="$t('#L_RATING_DUBIOUS_HINT')">
                  <i class="fas fa-exclamation-triangle" aria-hidden="true"></i>
                  {{ $t("#L_RATING_DUBIOUS") }}
                </span>
                <!-- SteamID — в меню «⋯», а не строкой в шапке. -->
                <span v-if="moreItems.length" class="profile-more" ref="more">
                  <button
                    ref="moreTrigger"
                    type="button"
                    class="profile-more__trigger"
                    :class="{ open: moreOpen }"
                    :title="$t('#L_PROFILE_MORE')"
                    :aria-label="$t('#L_PROFILE_MORE')"
                    aria-haspopup="menu"
                    :aria-expanded="moreOpen ? 'true' : 'false'"
                    @click="moreOpen ? closeMore(true) : openMore(0)"
                    @keydown.down.prevent="openMore(0)"
                    @keydown.up.prevent="openMore(-1)"
                  >
                    <i class="fas fa-ellipsis" aria-hidden="true"></i>
                  </button>
                  <transition name="profile-more">
                    <div
                      v-if="moreOpen"
                      class="profile-more__menu"
                      role="menu"
                      :aria-label="$t('#L_PROFILE_MORE')"
                      @keydown.esc.prevent.stop="closeMore(true)"
                      @keydown.down.prevent="moveMore(1)"
                      @keydown.up.prevent="moveMore(-1)"
                      @keydown.tab.prevent.stop="closeMore(true)"
                    >
                      <button
                        v-for="(item, index) in moreItems"
                        :key="item.id"
                        ref="moreOption"
                        type="button"
                        class="profile-more__item"
                        :style="{ '--i': index }"
                        role="menuitem"
                        @click="chooseMore(item)"
                        @focus="moreActive = index"
                      >
                        <span class="profile-more__icon" aria-hidden="true"><i :class="item.icon"></i></span>
                        <span>{{ $t(item.label) }}</span>
                      </button>
                    </div>
                  </transition>
                </span>
              </div>
            </div>
          </div>

          <!-- Вкладки сверху: окно читается слева направо целиком, без
               отдельной тёмной колонки сбоку. -->
          <nav class="profile__tabs" role="tablist" :aria-label="$t('#L_PROFILE_TITLE')">
            <button
              v-for="item in sections"
              :key="item.id"
              :ref="(el) => setTabRef(item.id, el)"
              type="button"
              role="tab"
              class="profile__tab"
              :class="{ 'is-active': section === item.id }"
              :aria-selected="section === item.id ? 'true' : 'false'"
              @click="section = item.id"
            >
              <i :class="item.icon" aria-hidden="true"></i>
              <span>{{ $t(item.label) }}</span>
              <span v-if="item.id === 'achievements'" class="profile__tab-count">{{ summary.unlocked }}/{{ summary.total }}</span>
            </button>
            <span v-if="ink" class="profile__tab-ink l-ink" :class="'is-' + inkMotion" :style="inkStyle" aria-hidden="true"></span>
          </nav>
        </header>

        <p v-if="ban && !targetSteamId" class="profile__ban" role="alert">
          <i class="fas fa-ban" aria-hidden="true"></i>
          <span>
            <b>{{ ban.expiresAt ? $t('#L_MOD_BANNED_UNTIL', formatModerationDate(ban.expiresAt)) : $t('#L_MOD_BANNED_FOREVER') }}</b>
            <span v-if="ban.reason"> {{ $t('#L_MOD_REASON', ban.reason) }}</span>
            <span class="profile__ban-note">{{ $t('#L_MOD_BANNED_NOTE') }}</span>
          </span>
        </p>

        <div class="profile__body">
          <!-- ================= Раздел «Профиль» =================
               Разделы показываются через v-show, переход Vue работает и с
               ним. Раздел уезжает в сторону, противоположную нажатой
               вкладке, новый приезжает с её стороны (l-slide-*, launcher.css);
               оба стоят в одной ячейке сетки .profile__body. -->
          <transition :name="'l-slide-' + slideDir">
            <div v-show="section === 'profile'" class="profile__overview">
              <section class="profile__column" aria-labelledby="profile-stats">
                <h3 class="profile__section-title" id="profile-stats">
                  {{ $t("#L_PROFILE_STATS") }}
                </h3>

                <dl class="profile__stats">
                  <div v-for="row in statRows" :key="row.icon" class="profile__stat" :class="{ 'is-wide': row.wide }" :title="row.value + ' — ' + row.label">
                    <i class="profile__stat-icon" :class="row.icon" aria-hidden="true"></i>
                    <dd class="profile__stat-value">{{ row.value }}</dd>
                    <dt class="profile__stat-label">{{ row.label }}</dt>
                  </div>
                </dl>
              </section>

              <section class="profile__column" aria-labelledby="profile-record">
                <h3 class="profile__section-title" id="profile-record">
                  {{ $t("#L_PROFILE_RECORD") }}
                </h3>

                <button
                  v-if="details.pinnedRun"
                  type="button"
                  class="runner-record"
                  :class="medalClass"
                  :title="$t('#L_PROFILE_OPEN_RUN')"
                  @click="openLink(details.pinnedRun.url)"
                >
                  <span class="runner-record__top">
                    <span class="runner-record__game">{{ details.pinnedRun.game }}</span>
                    <i class="fas fa-external-link-alt runner-record__open" aria-hidden="true"></i>
                  </span>
                  <span class="runner-record__category">{{ details.pinnedRun.category }}</span>
                  <span class="runner-record__bottom">
                    <strong class="runner-record__time">{{ formatRun(details.pinnedRun.seconds) }}</strong>
                    <RunPlace :place="details.pinnedRun.place" size="lg" />
                  </span>
                </button>
                <div v-else class="runner-record is-empty">
                  <i class="fas fa-stopwatch" aria-hidden="true"></i>
                  <span class="runner-record__game">{{ $t("#L_PROFILE_NO_RECORD") }}</span>
                  <span v-if="!targetSteamId" class="runner-record__category">{{ $t("#L_PROFILE_NO_RECORD_OWN") }}</span>
                </div>

                <div v-if="details.discord || details.youtube || details.twitch" class="profile__links">
                  <h3 class="profile__section-title profile__section-title--gap">{{ $t("#L_PROFILE_LINKS") }}</h3>
                  <div class="runner-socials">
                    <span v-if="details.discord" class="runner-socials__item is-discord" :title="'Discord: ' + details.discord"><i class="fab fa-discord" aria-hidden="true"></i><span class="runner-socials__name">{{ details.discord }}</span></span>
                    <button v-if="details.youtube" type="button" class="runner-socials__item is-youtube" :title="'YouTube: ' + channelLabel('youtube')" @click="openLink(details.youtube)"><i class="fab fa-youtube" aria-hidden="true"></i><span class="runner-socials__name">{{ channelLabel('youtube') }}</span></button>
                    <button v-if="details.twitch" type="button" class="runner-socials__item is-twitch" :title="'Twitch: ' + channelLabel('twitch')" @click="openLink(details.twitch)"><i class="fab fa-twitch" aria-hidden="true"></i><span class="runner-socials__name">{{ channelLabel('twitch') }}</span></button>
                  </div>
                </div>
              </section>
            </div>
          </transition>

          <!-- ================= Раздел «Достижения» ================= -->
          <transition :name="'l-slide-' + slideDir">
            <section
              v-show="section === 'achievements'"
              aria-labelledby="profile-ach"
            >
              <div class="profile__section-head">
                <h3 class="profile__section-title" id="profile-ach">
                  {{ $t("#L_PROFILE_ACHIEVEMENTS") }}
                </h3>
                <span class="profile__counter">{{ counterText }}</span>
              </div>

              <div
                class="profile__meter"
                :class="{ 'is-complete': complete }"
                role="progressbar"
                :aria-valuenow="summary.unlocked"
                aria-valuemin="0"
                :aria-valuemax="summary.total"
                :aria-label="counterText"
              >
                <div class="profile__meter-fill" :style="{ width: summary.percent + '%' }"></div>
              </div>

              <ul class="profile__achievements" aria-labelledby="profile-ach">
                <li
                  v-for="item in achievements"
                  :key="item.id"
                  class="profile__achievement"
                  :class="[item.tier && item.unlocked ? 'tier-' + item.tier : '', { locked: !item.unlocked, secret: item.secret && !item.hidden }]"
                >
                  <div class="profile__badge">
                    <i :class="item.icon" aria-hidden="true"></i>
                    <i
                      v-if="!item.unlocked"
                      class="profile__lock fas fa-lock"
                      aria-hidden="true"
                    ></i>
                  </div>

                  <div class="profile__achievement-content">
                    <span class="profile__achievement-title">{{ $t(item.title) }}</span>
                    <span class="profile__achievement-text">{{ describe(item) }}</span>
                    <div
                      v-if="!item.unlocked && item.goal"
                      class="profile__achievement-meter"
                      aria-hidden="true"
                    >
                      <div
                        class="profile__achievement-meter-fill"
                        :style="{ width: Math.round(item.progress * 100) + '%' }"
                      ></div>
                    </div>
                  </div>

                  <div v-if="item.unlocked" class="profile__done">
                    <span class="profile__state is-done">
                      <i class="fas fa-check" aria-hidden="true"></i>
                      {{ $t("#L_PROFILE_UNLOCKED") }}
                    </span>
                    <span v-if="!targetSteamId && item.at" class="profile__done-at">{{ formatAchievedAt(item.at) }}</span>
                  </div>
                  <span v-else class="profile__state">{{ progressText(item) }}</span>
                </li>
              </ul>
            </section>
          </transition>

          <!-- ================= Раздел «Настройки» ================= -->
          <transition :name="'l-slide-' + slideDir">
            <div v-if="!targetSteamId" v-show="section === 'settings'" class="profile__settings">
              <RunnerSettings
                :account="account"
                :details="details"
                :deletionAt="deletionAt"
                :hideSteamId="hideSteamId"
                :active="section === 'settings'"
                :mute="mute"
                :perks="perks"
                :subscriberBadge="subscriber"
                :stylePreset="stylePreset"
                :telegram="telegram"
                :discordLink="discordLink"
                @updated="applySnapshot"
                @toggle-id="$emit('toggle-id')"
                @copy="copy"
                @open-steam="openSteam"
                @privacy="openPrivacy"
                @open-sponsors="$emit('open-sponsors')"
              />
            </div>
          </transition>
        </div>
      </template>
    </div>
  </div>
</template>

<script>
import launcherData from "./launcher-data";
import RunnerSettings from "./RunnerSettings";
import RunPlace from "./RunPlace";
import NickName from "./NickName";
import AvatarFrame from "./AvatarFrame";
import { trapFocus } from "./focus-trap";
import orderedRefs from "@/utils/orderedRefs";

import { formatRun } from "./run-time";
import { inkBox, direction } from "./tab-ink";

const DEVELOPER_BADGE = require("@/assets/launcher/badges/developer.png");
const BUGHUNTER_BADGE = require("@/assets/launcher/badges/bughunter.png");
const BOOSTER_BADGE = require("@/assets/launcher/badges/booster.png");
const anniversaryBadges = require("../../../shared/anniversary-badges");
const ANNIVERSARY_BADGES = anniversaryBadges.IDS.map((id) => require("@/assets/launcher/badges/" + id + ".png"));
const { containsLink } = require("../../../shared/link-guard");
const profileLinks = require("../../../shared/profile-links");
const { pronounLabel, normalizePronouns } = require("../../../shared/pronouns");

const { ipcRenderer, clipboard } = window.hlsrElectron;

// Ошибки входа, у которых есть свой текст; остальные — «нет связи».
const LOGIN_ERRORS = Object.freeze({
  RUNNER_SESSION_EXPIRED: "#L_RUNNER_SESSION_EXPIRED",
  RUNNER_CLIENT_MODIFIED: "#L_RUNNER_CLIENT_MODIFIED",
  RUNNER_CLIENT_UNVERIFIED: "#L_RUNNER_CLIENT_UNVERIFIED",
  RUNNER_CLIENT_OUTDATED: "#L_RUNNER_CLIENT_OUTDATED",
  RUNNER_ATTEST_FAILED: "#L_RUNNER_ATTEST_FAILED",
});

export default {
  name: "ProfileModal",
  components: { RunnerSettings, RunPlace, NickName, AvatarFrame },
  emits: ["close", "notify", "toggle-id", "server-hide-id", "open-sponsors"],
  props: {
    targetSteamId: { type: String, default: "" },
    // "settings" открывает свой профиль сразу на настройках.
    initialSection: { type: String, default: "profile" },
    hideSteamId: {
      type: Boolean,
      default: false,
    },
  },
  data() {
    return {
      LOGIN_ERRORS,
      section: this.initialSection === "settings" && !this.targetSteamId ? "settings" : "profile",
      account: {},
      consentAccepted: false,
      details: {},
      badges: [],
      nick: null,
      // Подписка активна сейчас: значок у ника и голубая рамка окна.
      subscriber: false,
      boosterSince: null,
      supporterSince: null,
      customBadges: [],
      avatarFrame: null,
      deletionAt: null,
      ban: null,
      mute: null,
      // Свой профиль: права поддержавшего, выбранная заготовка подсветки и
      // привязка Telegram (вкладка «Настройки»).
      perks: { styles: false, subscriber: null },
      stylePreset: null,
      telegram: { linked: false, username: null },
      discordLink: { linked: false, username: null, boosterSince: null },
      missing: false,
      viewLoading: !!this.targetSteamId,
      viewFailed: false,
      // Чужой профиль открыт без входа: окно показывает вход.
      guestView: false,
      loginStatus: "signed-out",
      loginCode: null,
      loginError: null,
      loginSuccess: false,
      successTimer: null,
      achievements: [],
      summary: { unlocked: 0, total: 0, percent: 0 },
      stats: { totalSeconds: 0, gamesPlayed: 0, launches: 0, best: null },
      // Серия входов { days, best } — только у своего профиля.
      streak: null,
      moreOpen: false,
      moreActive: 0,
      firstRunAt: null,
      lastFocused: null,
      ink: null,
      inkMotion: "still",
      slideDir: "next",
      placeRefreshed: false,
      presence: { online: false, gameId: null, launcherId: null },
      rating: { score: 0, likes: 0, dislikes: 0, dubious: false },
      vote: { value: 0, nextAt: null },
      voting: false,
      voteNote: "",
    };
  },
  computed: {
    // Окно без профиля: вход, загрузка, ошибка или профиля нет.
    guestWindow() {
      return !this.account.signedIn || this.missing || this.viewLoading || this.viewFailed;
    },
    // Голосовать можно за чужой профиль, когда сам вошёл.
    canVote() {
      return !!this.targetSteamId && !this.guestView && !this.missing && this.account.signedIn;
    },
    ratingText() {
      return this.rating.score > 0 ? "+" + this.rating.score : String(this.rating.score);
    },
    sections() {
      if (!this.account.signedIn || this.missing) return [];
      return [
        { id: "profile", label: "#L_PROFILE_NAV_PROFILE", icon: "fas fa-user" },
        { id: "achievements", label: "#L_PROFILE_ACHIEVEMENTS", icon: "fas fa-trophy" },
        { id: "settings", label: "#L_PROFILE_NAV_SETTINGS", icon: "fas fa-sliders-h" },
      ].filter((item) => !this.targetSteamId || item.id !== "settings");
    },
    // Особый баннер у всех со значком «Разработчик HLSR».
    isDeveloper() {
      return this.badges.indexOf("developer") !== -1;
    },
    badgeList() {
      const known = {
        developer: { image: DEVELOPER_BADGE, label: this.$t("#L_RUNNER_DEVELOPER") },
        bughunter: { image: BUGHUNTER_BADGE, label: this.$t("#L_RUNNER_BUGHUNTER") },
        booster: { image: BOOSTER_BADGE, label: this.$t("#L_RUNNER_BOOSTER", this.boosterSince ? this.formatSince(this.boosterSince) : "") },
        verified: { icon: "fas fa-check-circle", label: this.$t("#L_RUNNER_VERIFIED") },
      };
      const lang = this.$t("#L_RUNNER_LOCALE") === "ru" ? "ru" : "en";
      // Годовщина в HLSR: сервер присылает один, текущий; со 2-го года — свечение.
      anniversaryBadges.IDS.forEach((id, i) => {
        const count = i + 1;
        known[id] = { image: ANNIVERSARY_BADGES[i], anniversary: true, glow: anniversaryBadges.GLOW[count] || null, label: this.$t(anniversaryBadges.labelKey(count, lang), String(count)) };
      });
      // Свои значки из админки: иконка SVG и подпись на языке интерфейса.
      // Стилизованный (themed) рисуется силуэтом в цвете темы, как встроенные.
      for (const badge of this.customBadges) {
        const note = badge.note[lang] || badge.note.ru;
        known[badge.id] = { image: badge.icon, themed: badge.themed === true, label: badge.label[lang] + (note ? " — " + note : "") };
      }
      return this.badges.filter((id) => known[id]).map((id) => Object.assign({ id }, known[id]));
    },
    medalClass() {
      const place = this.details.pinnedRun && this.details.pinnedRun.place;
      return { 1: "is-gold", 2: "is-silver", 3: "is-bronze" }[place] || "";
    },
    inkStyle() {
      return { left: this.ink.left + "px", right: this.ink.right + "px" };
    },
    displayName() {
      return this.account.personaName || this.$t("#L_SB_STEAM_ACCOUNT");
    },
    // Меню «⋯» в шапке. SteamID скрыт: чужой — если его владелец так решил
    // (details.hideSteamId с сервера), тогда в меню ничего нет; свой — по
    // настройке на этом компьютере, тогда нет только ссылки на Steam.
    moreItems() {
      const hidden = this.targetSteamId ? this.details.hideSteamId === true : this.hideSteamId;
      if (!this.account.steamId2 || (this.targetSteamId && hidden)) return [];
      const items = [{ id: "copy", icon: "fas fa-copy", label: "#L_PROFILE_COPY_ID" }];
      if (!hidden && this.account.steamId64) items.push({ id: "steam", icon: "fab fa-steam", label: "#L_PROFILE_MENU_STEAM" });
      return items;
    },
    // Все достижения, включая секретные: золотая рамка окна и полоса.
    complete() {
      return this.summary.total > 0 && this.summary.unlocked === this.summary.total;
    },
    connectionState() {
      const value = this.account && this.account.connection;
      return ["connected", "elevated", "offline", "checking"].indexOf(value) === -1
        ? "unknown"
        : value;
    },
    // Онлайн: свой — по связи с HLSR, чужой — по данным сервера
    presenceState() {
      if (!this.targetSteamId && this.connectionState === "checking") return "checking";
      return this.presence.online ? "online" : "offline";
    },
    playingGame() {
      return (this.presence.launcherId && launcherData.shortNames[this.presence.launcherId]) || "";
    },
    presenceText() {
      if (this.presenceState === "checking") return this.$t("#L_ST_CHECKING");
      if (this.playingGame) return this.$t("#L_PRESENCE_PLAYING", this.playingGame);
      return this.$t(this.presence.online ? "#L_PRESENCE_ONLINE" : "#L_PRESENCE_OFFLINE");
    },
    sinceText() {
      if (!this.firstRunAt) return this.$t("#L_PROFILE_SINCE_UNKNOWN");
      return this.$t("#L_PROFILE_SINCE", this.formatDate(this.firstRunAt));
    },
    // Описание, сохранённое до запрета ссылок, со ссылкой не показывается.
    shownStatus() {
      return containsLink(this.details.status || "") ? "" : this.details.status;
    },
    shownPronouns() {
      return pronounLabel(normalizePronouns(this.details.pronouns), this.$t("#L_RUNNER_LOCALE"));
    },
    windowLabel() {
      return this.$t("#L_PROFILE_TITLE");
    },
    counterText() {
      return this.$t("#L_PROFILE_COUNTER", `${this.summary.unlocked}/${this.summary.total}`);
    },
    statRows() {
      const rows = [
        {
          icon: "fas fa-clock",
          label: this.$t("#L_PROFILE_STAT_TOTAL_TIME"),
          value: this.formatHours(this.stats.totalSeconds),
        },
        {
          icon: "fas fa-play",
          label: this.$t("#L_PROFILE_STAT_LAUNCHES"),
          value: String(this.stats.launches || 0),
        },
        {
          icon: "fas fa-gamepad",
          label: this.$t("#L_PROFILE_STAT_GAMES"),
          value: String(this.stats.gamesPlayed || 0),
        },
        {
          icon: "fas fa-trophy",
          label: this.$t("#L_PROFILE_STAT_DONE"),
          value: this.$t("#L_PROFILE_STAT_OF", String(this.summary.unlocked), String(this.summary.total)),
        },
      ];

      if (this.streak)
        rows.push({
          icon: "fas fa-calendar-check",
          label: this.$t("#L_PROFILE_STAT_STREAK", String(this.streak.best)),
          value: this.daysText(this.streak.days),
          wide: true,
        });

      if (this.stats.best)
        rows.push({
          icon: "fas fa-fire",
          label: `${this.$t("#L_PROFILE_STAT_BEST")} · ${this.formatHours(this.stats.best.seconds)}`,
          value: this.gameName(this.stats.best),
          wide: true,
        });

      return rows;
    },
  },
  watch: {
    section(id, previous) {
      this.slideDir = direction(this.sections.map((item) => item.id), previous, id);
      this.$nextTick(() => this.measureInk(this.slideDir));
    },
    sections() {
      this.$nextTick(() => this.measureInk("still"));
    },
  },
  created() {
    this.tabEls = {};
  },
  methods: {
    // unix-секунды -> ДД.ММ.ГГГГ (день игрока), как дата подписки в настройках.
    formatSince(seconds) {
      const date = new Date(seconds * 1000);
      const pad = (n) => String(n).padStart(2, "0");
      return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
    },
    setTabRef(id, el) {
      if (el) this.tabEls[id] = el;
      else delete this.tabEls[id];
    },
    // Полоса под активной вкладкой перетекает к новой (tab-ink.js): так
    // видно, откуда и куда переключились. При изменении размеров окна —
    // без анимации.
    measureInk(motion) {
      const el = this.tabEls[this.section];
      const box = inkBox(el && el.parentElement, el);
      if (!box) return;
      this.inkMotion = this.ink && typeof motion === "string" ? motion : "still";
      this.ink = box;
    },
    // Закреплённые до появления места забеги его не знают: список рекордов
    // заодно обновляет место и ссылку закреплённого.
    refreshPinnedPlace() {
      const run = this.details.pinnedRun;
      if (this.targetSteamId || this.placeRefreshed || !run || run.place || !this.details.speedrun) return;
      this.placeRefreshed = true;
      ipcRenderer.invoke("runner-speedrun-bests").catch(() => {});
    },
    describe(item) {
      return item.goal ? this.$t(item.description, String(item.goal)) : this.$t(item.description);
    },
    progressText(item) {
      if (!item.goal) return this.$t("#L_PROFILE_LOCKED");
      // Единицы (запуски, часы, карты) уже названы в описании ачивки.
      return `${item.value || 0}/${item.goal}`;
    },
    formatHours(seconds) {
      const value = Number(seconds) || 0;
      const hours = Math.floor(value / 3600);
      const minutes = Math.floor((value % 3600) / 60);

      if (hours <= 0) return this.$t("#L_PROFILE_MINUTES", String(minutes));
      return this.$t("#L_PROFILE_HOURS", `${hours}`, `${minutes}`);
    },
    formatDate(iso) {
      const date = new Date(iso);
      if (isNaN(date.getTime())) return "";

      const pad = (n) => String(n).padStart(2, "0");
      return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
    },
    gameName(best) {
      const names = launcherData.shortNames || {};
      return names[best.launcherId] || best.gameId;
    },
    copy(value) {
      if (!value) return;
      clipboard.writeText(String(value));
      this.$emit("notify", "#L_SB_COPIED");
    },
    openSteam() {
      if (this.account.steamId64) this.openLink("https://steamcommunity.com/profiles/" + this.account.steamId64);
    },
    // «3 дня», «5 дней» / «1 day», «5 days».
    daysText(count) {
      const form = new Intl.PluralRules(this.$t("#L_RUNNER_LOCALE")).select(count);
      return this.$t(form === "one" ? "#L_PROFILE_DAYS_ONE" : form === "few" ? "#L_PROFILE_DAYS_FEW" : "#L_PROFILE_DAYS_MANY", String(count));
    },
    // Меню «⋯» — как меню игры (GameMenu.vue): стрелки, Esc и Tab.
    openMore(index) {
      this.moreOpen = true;
      this.moreActive = index < 0 ? this.moreItems.length - 1 : index;
      this.$nextTick(() => {
        document.addEventListener("mousedown", this.onMoreDocumentDown, true);
        this.focusMore();
      });
    },
    closeMore(returnFocus) {
      if (!this.moreOpen) return;
      this.moreOpen = false;
      document.removeEventListener("mousedown", this.onMoreDocumentDown, true);
      if (returnFocus && this.$refs.moreTrigger) this.$refs.moreTrigger.focus();
    },
    onMoreDocumentDown(event) {
      if (this.$refs.more && this.$refs.more.contains(event.target)) return;
      this.closeMore(false);
    },
    focusMore() {
      const target = orderedRefs(this.$refs.moreOption)[this.moreActive];
      if (target) target.focus();
    },
    moveMore(delta) {
      const total = this.moreItems.length;
      this.moreActive = (this.moreActive + delta + total) % total;
      this.focusMore();
    },
    chooseMore(item) {
      this.closeMore(true);
      if (item.id === "copy") this.copy(this.account.steamId2);
      if (item.id === "steam") this.openSteam();
    },
    applySnapshot(data) {
      if (!data || data.status === "error") {
        if (this.targetSteamId && !this.guestView) this.viewFailed = true;
        else this.loginError = "RUNNER_NETWORK";
        return;
      }
      this.missing = data.missing === true;
      this.account = data.account || {};
      this.achievements = data.achievements || [];
      this.summary = data.summary || this.summary;
      this.stats = data.stats || this.stats;
      this.streak = (!this.targetSteamId && data.streak) || null;
      this.firstRunAt = data.firstRunAt || null;
      this.details = data.details || {};
      this.presence = data.presence || { online: false, gameId: null, launcherId: null };
      this.rating = data.rating || { score: 0, likes: 0, dislikes: 0, dubious: false };
      this.vote = data.vote || { value: 0, nextAt: null };
      // Своё значение «Скрыть SteamID» на сервере: оболочка сверяет его с
      // настройкой на компьютере (LauncherShell.onServerHideSteamId).
      if (!this.targetSteamId && this.account.signedIn) this.$emit("server-hide-id", this.details.hideSteamId);
      this.badges = data.badges || [];
      this.nick = data.nick || null;
      this.subscriber = data.subscriber === true;
      this.boosterSince = data.boosterSince || null;
      this.supporterSince = data.supporterSince || null;
      this.customBadges = data.customBadges || [];
      this.avatarFrame = data.avatarFrame || null;
      this.deletionAt = data.deletionAt || null;
      if (!this.targetSteamId) {
        this.ban = data.ban || null;
        this.mute = data.mute || null;
        this.perks = data.perks || { styles: false, subscriber: null };
        this.stylePreset = data.stylePreset || null;
        this.telegram = data.telegram || { linked: false, username: null };
        this.discordLink = data.discordLink || { linked: false, username: null, boosterSince: null };
      }
      this.loginStatus = data.status || "signed-out";
      this.loginCode = data.userCode || null;
      this.loginError = data.error || null;
      this.$nextTick(this.measureInk);
      this.refreshPinnedPlace();
    },
    async load() {
      if (this.targetSteamId) { this.viewLoading = true; this.viewFailed = false; }
      try {
        if (this.targetSteamId) {
          const own = await ipcRenderer.invoke("runner-state");
          this.guestView = !(own && own.account && own.account.signedIn);
          if (this.guestView) { this.applySnapshot(own); return; }
        }
        this.applySnapshot(await ipcRenderer.invoke(this.targetSteamId ? "runner-profile-view" : "profile-get", this.targetSteamId || undefined));
        if (!this.targetSteamId) ipcRenderer.invoke("runner-sync").catch(() => {});
      } catch (error) {
        if (this.targetSteamId) this.viewFailed = true;
        else this.loginError = "RUNNER_NETWORK";
      } finally {
        this.viewLoading = false;
      }
    },
    onProfilePushed(event, value) {
      if (this.targetSteamId && !this.guestView) return;
      this.applySnapshot(value);
      // Вошли из окна чужого профиля — теперь его можно загрузить.
      if (this.guestView && value && value.account && value.account.signedIn) { this.guestView = false; this.load(); }
    },
    // Вход сам означает согласие с условиями, статистика включена по
    // умолчанию; выключить её можно в настройках («Техническая статистика»).
    async signIn() { if (!this.consentAccepted) return; const result = await ipcRenderer.invoke("runner-login", true); this.applySnapshot(result); },
    voteTitle(value) {
      if (this.vote.nextAt && this.vote.value !== 0) return this.$t("#L_RATING_NEXT", this.formatVoteTime(this.vote.nextAt));
      return this.$t(value === 1 ? (this.vote.value === 1 ? "#L_RATING_UNLIKE" : "#L_RATING_LIKE") : this.vote.value === -1 ? "#L_RATING_UNDISLIKE" : "#L_RATING_DISLIKE");
    },
    formatVoteTime(iso) {
      const at = new Date(iso);
      return Number.isNaN(at.getTime()) ? "" : at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    },
    // Повторное нажатие на свой голос снимает его. Менять голос можно раз
    // в 4 часа: сервер отвечает cooldown и время, когда можно снова. Лайков
    // за 4 часа — не больше пяти разным профилям (like-limit).
    async castVote(value) {
      if (!this.canVote || this.voting) return;
      const next = this.vote.value === value ? 0 : value;
      this.voting = true;
      this.showVoteNote("");
      try {
        const result = await ipcRenderer.invoke("runner-rate", this.targetSteamId, next);
        if (!result || result.status === "error") {
          this.showVoteNote(this.$t("#L_RATING_FAILED"));
          return;
        }
        this.rating = result.rating;
        this.vote = result.vote;
        if (result.status === "cooldown") this.showVoteNote(this.$t("#L_RATING_NEXT", this.formatVoteTime(result.vote.nextAt)));
        if (result.status === "like-limit") this.showVoteNote(this.$t("#L_RATING_LIKE_LIMIT", this.formatVoteTime(result.likesAt)));
      } catch (error) {
        this.showVoteNote(this.$t("#L_RATING_FAILED"));
      } finally {
        this.voting = false;
      }
    },
    // Подсказка висит поверх строки под ником, поэтому сама прячется.
    showVoteNote(text) {
      clearTimeout(this.voteNoteTimer);
      this.voteNote = text;
      if (text) this.voteNoteTimer = setTimeout(() => { this.voteNote = ""; }, 4000);
    },
    async cancelLogin() { this.applySnapshot(await ipcRenderer.invoke("runner-login-cancel")); },
    onLoginSuccess() { this.loginSuccess = true; if (this.successTimer) clearTimeout(this.successTimer); this.successTimer = setTimeout(() => { this.loginSuccess = false; }, 4000); },
    openPrivacy() { ipcRenderer.invoke("open-external-link", "privacy"); },
    // «8 ноября 2026, 02:41» / «Nov 8, 2026, 2:41 AM» — по часам игрока.
    formatAchievedAt(iso) {
      const at = new Date(iso);
      if (this.$t("#L_RUNNER_LOCALE") !== "ru") return at.toLocaleString("en-US", { dateStyle: "medium", timeStyle: "short" });
      return `${at.toLocaleDateString("ru-RU", { day: "numeric", month: "long" })} ${at.getFullYear()}, ${at.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" })}`;
    },
    formatModerationDate(iso) {
      const locale = this.$t("#L_RUNNER_LOCALE") === "ru" ? "ru-RU" : "en-GB";
      return new Date(iso).toLocaleString(locale, { day: "numeric", month: "long", year: "numeric", hour: "2-digit", minute: "2-digit" });
    },
    openTerms() { ipcRenderer.invoke("open-external-link", "terms"); },
    openLink(url) { ipcRenderer.invoke("runner-open-link", url); },
    // Ник канала из адреса; у канала по ID (UC…) ника в адресе нет.
    channelLabel(field) {
      return profileLinks.channelName(field, this.details[field] || "") || (field === "youtube" ? "YouTube" : "Twitch");
    },
    countryName(code) { try { return code ? new Intl.DisplayNames([this.$t("#L_RUNNER_LOCALE")], { type: "region" }).of(code) : ""; } catch (error) { return code || ""; } },
    formatRun,
    onKeydown(e) {
      if (e.key === "Tab") return trapFocus(this.$refs.window, e);
      if (e.key !== "Escape") return;
      e.preventDefault();

      this.$emit("close");
    },
  },
  mounted() {
    ipcRenderer.on("runner-profile-state", this.onProfilePushed);
    ipcRenderer.on("runner-login-success", this.onLoginSuccess);
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);

    this.load();
    window.addEventListener("resize", this.measureInk);

    this.$nextTick(() => {
      if (this.$refs.closeButton) this.$refs.closeButton.focus();
    });
  },
  beforeUnmount() {
    ipcRenderer.removeListener("runner-profile-state", this.onProfilePushed);
    ipcRenderer.removeListener("runner-login-success", this.onLoginSuccess);
    if (this.successTimer) clearTimeout(this.successTimer);
    clearTimeout(this.voteNoteTimer);
    document.removeEventListener("mousedown", this.onMoreDocumentDown, true);
    document.removeEventListener("keydown", this.onKeydown);
    window.removeEventListener("resize", this.measureInk);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
@use "./round-dot" as *;
@use "./neon" as *;

// Рамка окна подписчика (.profile__window.is-subscriber): бегущий голубой
// неон внутри окна (neon.scss) и неподвижное свечение — внутреннее (под
// содержимым) и наружное, бирюзовое ближе к краю, синее дальше. Свечение
// остаётся и без анимации.
$subscriber-ring: inset 0 0 16px rgba(56, 189, 248, 0.22), 0 0 18px rgba(56, 189, 248, 0.35), 0 0 42px rgba(59, 130, 246, 0.18);
$complete-ring: inset 0 0 16px rgba(245, 197, 66, 0.22), 0 0 18px rgba(245, 197, 66, 0.4), 0 0 42px rgba(217, 140, 30, 0.2);
@include neon(profile-neon, #38bdf8, #60a5fa, rgba(56, 189, 248, 0.2));

/* ---- Вход и пустой профиль ---- */
.profile__window.is-guest { width: min(560px, calc(100% - 64px)); height: auto; max-height: calc(100% - 64px); }
.profile__guest { padding: 32px; overflow-y: auto; }
.runner-signin { color: var(--l-text); }
.runner-signin h2 { font-size: 19px; line-height: 24px; font-weight: 700; margin: 16px 0 12px; }
.runner-signin p { color: var(--l-text-dim); font-size: 13px; line-height: 24px; margin: 12px 0; }
.runner-signin__steam { width: 56px; height: 56px; display: flex; align-items: center; justify-content: center; color: var(--l-text-dim); font-size: 32px; border-radius: var(--l-radius); background: rgba(255,255,255,.04); box-shadow: inset 0 0 0 1px var(--l-border); }
.runner-signin__consent { position: relative; display: flex; align-items: flex-start; gap: 10px; max-width: 420px; margin: 24px 0 0; color: var(--l-text-dim); font-size: 12.5px; line-height: 20px; text-align: left; cursor: pointer; }
.runner-signin__consent input { position: absolute; opacity: 0; width: 18px; height: 18px; margin: 1px 0 0; }
.runner-signin__consent .runner-signin__privacy { margin-top: 0; }
.runner-signin__box { flex: none; display: grid; place-items: center; width: 18px; height: 18px; margin-top: 1px; border-radius: 5px; box-shadow: inset 0 0 0 1.5px var(--l-border-strong); color: transparent; font-size: 10px; transition: background var(--l-fast), color var(--l-fast); }
.runner-signin__consent input:checked + .runner-signin__box { background: var(--accent); box-shadow: none; color: var(--accent-contrast-text); }
.runner-signin__consent input:focus-visible + .runner-signin__box { outline: 2px solid var(--accent); outline-offset: 2px; }
.runner-signin__button:disabled { opacity: .45; cursor: default; }
.runner-signin p.runner-signin__local { max-width: 420px; margin: 16px 0 0; color: var(--l-text-faint); font-size: 12px; line-height: 18px; }
.runner-signin__button { display: inline-flex; align-items: center; gap: 8px; min-height: 48px; color: var(--l-text); background: rgba(255,255,255,.05); border: 0; box-shadow: inset 0 0 0 1px var(--l-border-strong); border-radius: var(--l-radius-sm); padding: 12px 20px; margin-top: 16px; font-family: inherit; font-size: 13.5px; font-weight: 600; line-height: 24px; cursor: pointer; transition: background var(--l-fast); }
.runner-signin__button.is-primary { background: var(--accent); color: var(--accent-contrast-text); box-shadow: none; }
.runner-signin__button.is-primary:hover { background: var(--accent-hover); }
.runner-signin__button:focus-visible, .runner-signin__privacy:focus-visible, .runner-socials button:focus-visible, .runner-record:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.runner-signin__privacy { color: var(--l-accent); background: none; border: 0; padding: 0; margin-top: 16px; font-family: inherit; font-size: 12.5px; line-height: 20px; cursor: pointer; text-decoration: underline; text-underline-offset: 3px; }
.runner-signin__actions { display: flex; flex-wrap: wrap; align-items: center; gap: 16px 24px; margin-top: 16px; }
.runner-signin__actions .runner-signin__button, .runner-signin__actions .runner-signin__privacy { margin-top: 0; }
.profile__ban { display: flex; gap: 12px; align-items: flex-start; margin: 0 32px 12px; padding: 12px 16px; border-radius: var(--l-radius-sm); background: rgba(255, 92, 92, .12); box-shadow: inset 0 0 0 1px rgba(255, 92, 92, .35); color: var(--l-text); font-size: 12.5px; line-height: 20px; }
.profile__ban > i { margin-top: 3px; color: #ff6b6b; }
.profile__ban-note { display: block; color: var(--l-text-dim); }
.profile__pending { display: flex; flex-direction: column; align-items: center; text-align: center; }
.profile__pending p { text-wrap: balance; }
.profile__spinner { margin: 8px 0 4px; color: var(--l-text-faint); font-size: 20px; }
.runner-signin__code { display: block; font: 600 24px/32px monospace; letter-spacing: 4px; color: var(--l-text); margin: 20px 0; }

.runner-welcome { position: absolute; z-index: 3; left: 50%; bottom: 24px; transform: translateX(-50%); display: flex; align-items: center; gap: 8px; padding: 8px 16px 8px 12px; color: #4ad07a; font-size: 12.5px; line-height: 20px; white-space: nowrap; background: var(--l-modal-bg); border-radius: var(--l-radius); box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 8px 24px rgba(0, 0, 0, 0.45); pointer-events: none; }
.runner-welcome svg { width: 24px; height: 24px; fill: none; stroke: currentColor; stroke-width: 2; }
.runner-welcome path { stroke-dasharray: 30; stroke-dashoffset: 30; animation: runner-check .5s .1s var(--l-ease) forwards; }
.runner-welcome circle { opacity: .45; }
.runner-welcome-enter-active, .runner-welcome-leave-active { transition: opacity var(--l-med), transform var(--l-med); }
.runner-welcome-enter-from, .runner-welcome-leave-to { opacity: 0; transform: translate(-50%, var(--l-rise)); }
@keyframes runner-check { to { stroke-dashoffset: 0; } }
@keyframes profile-badge-glow {
  from { filter: drop-shadow(0 0 2px rgba(var(--glow), 0.35)); }
  to { filter: drop-shadow(0 0 6px rgba(var(--glow), 0.95)); }
}
/* Без анимаций (система или переключатель в настройках) свечение стоит. */
@media (prefers-reduced-motion: reduce) {
  .profile__badge-chip > img { animation: none !important; }
}
.launcher-root.no-anim .profile__badge-chip > img { animation: none !important; }
@media (prefers-reduced-motion: reduce) { .runner-welcome path { animation: none; stroke-dashoffset: 0; } }

/* ---- Статус, примечание, ссылки ---- */
.runner-status { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 12px; margin: 0; color: var(--l-text); font-size: 13.5px; line-height: 20px; overflow-wrap: anywhere; }
.runner-status small { color: var(--l-text-faint); font-size: 12px; }
.runner-profile__note { color: var(--l-text-faint); font-size: 12px; line-height: 20px; margin: 12px 0 0; }

/* Ссылки одним рядом, поровну ширины: столбиком три ссылки не помещались
   в окно, и профиль начинал прокручиваться. Длинный ник обрезается
   многоточием, целиком он в подсказке. */
.runner-socials { display: flex; gap: 8px; }
.runner-socials__item { flex: 1 1 0; min-width: 0; display: flex; align-items: center; gap: 10px; min-height: 44px; padding: 0 14px; border: 0; border-radius: var(--l-radius-sm); background: rgba(255,255,255,.03); box-shadow: inset 0 0 0 1px var(--l-border); color: var(--l-text-dim); font-family: inherit; font-size: 13px; line-height: 20px; text-align: left; }
.runner-socials__name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.runner-socials__item i { flex: none; width: 20px; font-size: 16px; text-align: center; }
.runner-socials__item.is-discord i { color: #8c9eff; }
.runner-socials__item.is-youtube i { color: #ff5252; }
.runner-socials__item.is-twitch i { color: #b18cff; }
button.runner-socials__item { cursor: pointer; transition: background var(--l-fast), color var(--l-fast), box-shadow var(--l-fast); }
button.runner-socials__item:hover { background: rgba(255,255,255,.06); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 35%, var(--l-border)); color: var(--l-text); }

/* ---- Закреплённый забег ----
   Крупная карточка: время — главное, место с лямбдой рядом. Первые три
   места подсвечивают карточку цветом своей лямбды. */
.runner-record {
  --record-tint: var(--l-accent);
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 4px;
  width: 100%;
  padding: 20px;
  border: 0;
  border-radius: var(--l-radius);
  background:
    radial-gradient(120% 140% at 100% 0%, color-mix(in srgb, var(--record-tint) 14%, transparent) 0%, transparent 60%),
    rgba(255,255,255,.03);
  /* Прозрачная тень снаружи — запас области перерисовки (см. достижения). */
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--record-tint) 22%, var(--l-border)), 0 0 0 1px transparent;
  color: var(--l-text-dim);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: box-shadow var(--l-fast), background-color var(--l-fast);
  overflow: hidden;

  &.is-gold { --record-tint: #f5c542; }
  &.is-silver { --record-tint: #cfd8e3; }
  &.is-bronze { --record-tint: #d98c4a; }

  /* Наведение только подсвечивает рамку и фон: карточка не прыгает. */
  &:not(.is-empty):hover {
    background-color: rgba(255,255,255,.05);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--record-tint) 45%, var(--l-border)), 0 0 0 1px transparent;
  }

  &.is-empty {
    align-items: center;
    justify-content: center;
    padding: 20px;
    text-align: center;
    cursor: default;
    background: rgba(255,255,255,.02);
    box-shadow: inset 0 0 0 1px var(--l-border);

    > i { color: var(--l-text-faint); font-size: 22px; margin-bottom: 8px; }
    /* Заголовок карточки забега тянется по ширине (flex: 1); в столбике
       пустой карточки он тянулся по высоте и уводил всё наверх. */
    .runner-record__game { flex: none; }
    .runner-record__category { max-width: 340px; text-wrap: balance; }
  }

  &__top { display: flex; align-items: center; gap: 12px; }
  &__game { flex: 1; min-width: 0; color: var(--l-text); font-size: 14px; font-weight: 600; line-height: 20px; }
  &__open { color: var(--l-text-faint); font-size: 11px; transition: color var(--l-fast); }
  &:hover &__open { color: var(--l-text); }
  &__category { color: var(--l-text-dim); font-size: 12.5px; line-height: 20px; }
  &__bottom { display: flex; align-items: center; justify-content: space-between; gap: 16px; margin-top: auto; padding-top: 12px; }
  &__time { color: var(--l-text); font-size: 30px; font-weight: 700; line-height: 36px; letter-spacing: 0.01em; font-variant-numeric: tabular-nums; }
}

.profile {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 70;

  &__window {
    position: relative;
    width: min(1040px, calc(100% - 64px));
    height: min(720px, calc(100% - 64px));
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }

  /* Профиль подписчика: бегущая голубая рамка (.neon поверх баннера, клики
     пропускает) и свечение тенью снаружи окна; размер окна не меняется. */
  &__window.is-subscriber {
    box-shadow: $subscriber-ring, 0 24px 72px rgba(0, 0, 0, 0.6);
  }

  /* 100 % достижений: та же бегущая по часовой рамка, но золотая, и золотое
     свечение. У подписчика со всеми достижениями — золото. */
  &__window.is-complete {
    box-shadow: $complete-ring, 0 24px 72px rgba(0, 0, 0, 0.6);
  }

  &__window.is-complete .neon::before {
    background: conic-gradient(from 0deg, rgba(245, 197, 66, 0.2), rgba(245, 197, 66, 0.2) 30%, #ffd86b 55%, #f5a623 75%, rgba(245, 197, 66, 0.2));
  }

  /* Правый край крестика — на линии правого края содержимого (32 px). */
  &__close {
    position: absolute;
    top: 24px;
    right: 32px;
    width: 40px;
    height: 40px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(0, 0, 0, 0.32);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);
    z-index: 4;

    &:hover {
      background: rgba(255, 255, 255, 0.1);
      color: var(--l-text);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }
  }

  /* ---- Шапка ---- */

  &__hero {
    position: relative;
    flex: none;
    padding: 0 32px;
    box-shadow: inset 0 -1px 0 var(--l-modal-border);
  }

  /* Баннер: мягкое свечение акцента и крупная бледная лямбда справа —
     фирменная примета, а не картинка, которую надо грузить. */
  &__banner {
    position: absolute;
    inset: 0 0 auto;
    height: 112px;
    background:
      radial-gradient(70% 160% at 0% 0%, color-mix(in srgb, var(--l-accent) 30%, transparent) 0%, transparent 70%),
      radial-gradient(60% 140% at 100% 100%, color-mix(in srgb, var(--l-accent) 14%, transparent) 0%, transparent 70%),
      linear-gradient(180deg, color-mix(in srgb, var(--l-accent) 12%, var(--l-modal-bg)) 0%, var(--l-modal-bg) 100%);
    overflow: hidden;

    &::after {
      content: "";
      position: absolute;
      right: 72px;
      top: -36px;
      width: 180px;
      height: 188px;
      background: var(--l-text);
      opacity: 0.05;
      -webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 177 186'%3E%3Cpath d='M47.2 12.9v25.9h21.4l8.7 27.1-65.1 101h32l46.5-68.6 30.5 77 48-15.3-7.6-23.6-22.1 6.9-51.8-130.4z'/%3E%3C/svg%3E") center / contain no-repeat;
      mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 177 186'%3E%3Cpath d='M47.2 12.9v25.9h21.4l8.7 27.1-65.1 101h32l46.5-68.6 30.5 77 48-15.3-7.6-23.6-22.1 6.9-51.8-130.4z'/%3E%3C/svg%3E") center / contain no-repeat;
      transform: rotate(-12deg);
    }
  }

  /* Баннер разработчика: акцент со вторым оттенком, сетка из точек и
     светящийся знак «</>» вместо бледной лямбды. Неподвижный, рисуется
     целиком в CSS, картинки не грузятся. */
  &__banner.is-developer {
    --dev-tint: color-mix(in srgb, var(--l-accent) 40%, #a26bff);
    background:
      radial-gradient(70% 160% at 0% 0%, color-mix(in srgb, var(--l-accent) 42%, transparent) 0%, transparent 70%),
      radial-gradient(60% 160% at 100% 0%, color-mix(in srgb, var(--dev-tint) 48%, transparent) 0%, transparent 70%),
      linear-gradient(110deg, color-mix(in srgb, var(--l-accent) 18%, var(--l-modal-bg)) 0%,
        color-mix(in srgb, var(--dev-tint) 20%, var(--l-modal-bg)) 50%,
        color-mix(in srgb, var(--l-accent) 18%, var(--l-modal-bg)) 100%);

    /* Сетка из точек, тающая к низу баннера. */
    &::before {
      content: "";
      position: absolute;
      inset: 0;
      background-image: radial-gradient(circle, rgba(255, 255, 255, 0.16) 1px, transparent 1.5px);
      background-size: 16px 16px;
      -webkit-mask: linear-gradient(180deg, #000 0%, transparent 85%);
      mask: linear-gradient(180deg, #000 0%, transparent 85%);
    }

    /* Знак «</>» на месте лямбды: тот же угол и размер, но светится
       оттенком баннера. */
    &::after {
      top: -20px;
      right: 64px;
      width: 200px;
      height: 160px;
      background: var(--dev-tint);
      opacity: 0.16;
      -webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 120 96'%3E%3Cpath d='M34 18 6 48l28 30 9-9-19-21 19-21zM86 18l-9 9 19 21-19 21 9 9 28-30zM67 8 41 88l12 4L79 12z'/%3E%3C/svg%3E") center / contain no-repeat;
      mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 120 96'%3E%3Cpath d='M34 18 6 48l28 30 9-9-19-21 19-21zM86 18l-9 9 19 21-19 21 9 9 28-30zM67 8 41 88l12 4L79 12z'/%3E%3C/svg%3E") center / contain no-repeat;
      transform: rotate(-12deg);
      filter: drop-shadow(0 0 12px var(--dev-tint));
    }
  }

  /* Аватар прибит к верху: колонка с ником растёт вниз (описание,
     местоимения), а аватар остаётся на своём месте на краю баннера. */
  &__identity {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 16px;
    padding-top: 56px;
    padding-right: 56px;
  }

  /* С рамкой её 6 px встают на место кольца цвета окна вокруг аватара. */
  &__avatar-frame.is-framed &__avatar {
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
  }

  &__avatar {
    position: relative;
    flex: 0 0 112px;
    width: 112px;
    height: 112px;
    border-radius: 20px;
    background: var(--l-modal-bg);
    box-shadow: 0 0 0 6px var(--l-modal-bg), 0 12px 32px rgba(0, 0, 0, 0.45);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--l-text-faint);
    font-size: 40px;

    img {
      width: 100%;
      height: 100%;
      border-radius: inherit;
      object-fit: cover;
      display: block;
    }
  }

  &__dot {
    position: absolute;
    right: -4px;
    bottom: -4px;
    width: 20px;
    height: 20px;
    @include round-dot(var(--l-text-faint), var(--l-modal-bg), 4px);

    &.is-online { --dot-color: #4ad07a; }
    &.is-checking { --dot-color: #ffb37a; }
    &.is-offline { --dot-color: #8b939e; }
  }

  &__who {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    /* Ник опущен к нижнему краю баннера, ближе к плашке значков: строка
       ника 70–106 px, баннер кончается на 112 px. */
    padding-top: 14px;
  }

  &__name {
    /* Свечение оформленного ника выходит за буквы, а блок обрезает всё
       по краю ради многоточия: поля под свечение тут же съедаются
       отрицательным отступом, раскладка не меняется. */
    margin: -8px -14px;
    padding: 8px 14px;
    max-width: calc(100% + 28px);
    color: var(--l-text);
    font-size: 28px;
    font-weight: 700;
    line-height: 36px;
    letter-spacing: 0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Значки под ником одной плашкой, как в Discord: иконки рядом, имя
     значка — во всплывающей подсказке. Ник стоит на баннере вплотную к
     его нижнему краю, а плашка начинается сразу под краем (баннер 112 px,
     плашка с 116 px), поэтому граница баннера её не режет. */
  /* Поля плашки одинаковые со всех сторон: от значка до края и сверху,
     и сбоку 9.5 px, между значками — столько же. */
  &__badges {
    display: inline-flex;
    align-items: center;
    gap: 0;
    margin: 2px 0 0;
    padding: 4px;
    list-style: none;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &__badge-chip {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 6px;
    font-size: 16px;
    cursor: default;
    transition: background-color var(--l-fast);

    /* Значок «подтверждён» (глиф) в цвете темы. */
    &.is-verified { color: var(--l-accent); }

    /* Картинки со своим неоновым свечением: drop-shadow на значке 24 px
       «дышит» 2,4 с (размытого слоя на всё окно нет). Цвет задаёт --glow;
       у годовщин (is-glow) он приходит из anniversary-badges.js. */
    &.is-developer { --glow: 40, 140, 255; }
    &.is-bughunter { --glow: 250, 204, 21; }
    &.is-booster { --glow: 255, 79, 216; }

    &.is-developer > img,
    &.is-bughunter > img,
    &.is-booster > img,
    &.is-glow > img {
      filter: drop-shadow(0 0 3px rgba(var(--glow), 0.55));
      animation: profile-badge-glow 2.4s ease-in-out infinite alternate;
    }

    /* Глиф Font Awesome 7 вписан в свою строку (line-height: 1) ровно
       по центру, сдвигать его не нужно: любой сдвиг в пикселях на одном
       масштабе выравнивает, а на другом уводит. */
    > i {
      display: block;
      line-height: 1;
    }

    > img {
      display: block;
      width: 24px;
      height: 24px;
      object-fit: contain;
    }

    /* У годовщины в картинке поле под нарисованное свечение (у 9 и 10 оно
       шире всех): картинка крупнее, чтобы квадрат значка был ростом с
       соседние значки. */
    &.is-anniversary > img {
      width: 30px;
      height: 30px;
    }

    &:hover {
      background-color: rgba(255, 255, 255, 0.07);
    }
  }

  /* Стилизованный свой значок: от SVG берётся только форма, цвет — акцент
     темы, как у встроенных. Маска с data:-адресом, как и <img>, скрипты
     SVG не выполняет. */
  &__badge-mask {
    display: block;
    width: 16px;
    height: 16px;
    background-color: var(--l-accent);
    mask: var(--badge-icon) center / contain no-repeat;
  }

  &__meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
  }

  /* Иконка и текст стоят на общей базовой линии: при центрировании
     каждый округлялся до пикселя по-своему, и на 100 % и 125 % иконка
     уезжала от текста. Глиф FA и заглавные Rubik от базовой линии
     одинаковой высоты, так что центры совпадают. */
  &__since {
    display: flex;
    align-items: baseline;
    gap: 8px;
    color: var(--l-text-faint);
    font-size: 12.5px;
    line-height: 20px;

    i { font-size: 11px; }
  }

  /* ---- Вкладки ---- */

  &__tabs {
    position: relative;
    display: flex;
    gap: 8px;
    margin-top: 20px;
  }

  &__tab {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 48px;
    padding: 0 12px;
    border: 0;
    background: none;
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    line-height: 20px;
    cursor: pointer;
    transition: color var(--l-fast);

    i {
      font-size: 12px;
      color: var(--l-text-faint);
      transition: color var(--l-fast);
    }

    &:hover { color: var(--l-text); }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: -4px;
      border-radius: var(--l-radius-sm);
    }

    &.is-active {
      color: var(--l-text);

      i { color: var(--l-accent); }
    }
  }

  &__tab-count {
    min-width: 20px;
    padding: 0 6px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--l-text-dim);
    font-size: 11px;
    line-height: 18px;
    font-variant-numeric: tabular-nums;
  }

  /* Перетекает между вкладками (.l-ink в launcher.css). */
  &__tab-ink {
    bottom: 0;
    height: 2px;
    border-radius: 2px 2px 0 0;
    background: var(--l-accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 60%, transparent);
  }

  /* ---- Содержимое ---- */

  /* Разделы — страницы на всю ширину, каждая со своей прокруткой, как в
     iOS: при смене они стоят в одной ячейке и уезжают на одинаковую
     ширину (l-slide-*), а прокрутка одного раздела не сдвигает другой.
     Выезжающее за край обрезается. */
  &__body {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    overflow: hidden;

    > * {
      grid-area: 1 / 1;
      min-height: 0;
      box-sizing: border-box;
      padding: 28px 32px 32px;
      overflow-y: auto;
    }
  }

  /* Две равные колонки на общих строках: заголовки на одной линии, а блок
     забега — ровно той же высоты, что сетка сводки (пустой тоже). Колонки
     раскрыты в сетку (display: contents); место каждого ребёнка задано явно
     (колонка — по секции, строка — по номеру), поэтому число детей в колонке
     не сдвигает соседнюю. */
  &__overview {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    grid-template-rows: repeat(3, auto);
    column-gap: 32px;
    align-items: start;
    align-content: start;
  }

  &__column {
    display: contents;
  }

  &__column:nth-child(1) > * { grid-column: 1; }
  &__column:nth-child(2) > * { grid-column: 2; }
  &__column > :nth-child(1) { grid-row: 1; }
  &__column > :nth-child(2) { grid-row: 2; }
  &__column > :nth-child(3) { grid-row: 3; }

  // По содержимому, а не по высоте «Сводки» слева: она выросла в 0.4.9,
  // и карточка забега тянулась в пустое окно.
  &__overview .runner-record {
    align-self: start;
  }

  &__section-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
  }

  &__section-title {
    margin: 0 0 12px;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.12em;
    text-transform: uppercase;

    &--gap { margin-top: 24px; }
  }

  &__section-head &__section-title { margin-bottom: 0; }

  &__counter {
    color: var(--l-text-faint);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  &__stats {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }

  &__stat {
    display: grid;
    grid-template-columns: 32px minmax(0, 1fr);
    grid-template-rows: auto auto;
    column-gap: 12px;
    align-items: center;
    padding: 14px 16px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);

    &.is-wide,
    &:last-child:nth-child(odd) { grid-column: 1 / -1; }
  }

  &__stat-icon {
    grid-row: 1 / 3;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--l-radius-sm);
    background: var(--l-accent-soft);
    color: var(--l-accent);
    font-size: 13px;
  }

  &__stat-value {
    margin: 0;
    color: var(--l-text);
    font-size: 16px;
    font-weight: 700;
    line-height: 22px;
    font-variant-numeric: tabular-nums;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__stat-label {
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
  }

  &__meter {
    height: 4px;
    margin: 12px 0 20px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }

  &__meter-fill {
    height: 100%;
    background: var(--l-accent);
    transition: width var(--l-slow);
  }

  &__meter.is-complete &__meter-fill {
    background: linear-gradient(90deg, #f5a623, #ffd86b, #f5c542);
  }

  /* ---- Достижения: сетка карточек ---- */

  &__achievements {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    /* Все ряды одной высоты: иначе ряд с полоской прогресса выше, и
       полученное достижение рядом с ней казалось крупнее остальных. */
    grid-auto-rows: 1fr;
    gap: 12px;
  }

  /* Карточки одной высоты; у полученной нет полоски прогресса,
     поэтому содержимое стоит по центру, а не прижато к верху. Наведение —
     как у закреплённого забега: подсвечиваются рамка и фон, без подъёма. */
  &__achievement {
    display: grid;
    grid-template-columns: 44px minmax(0, 1fr) auto;
    align-items: center;
    gap: 16px;
    padding: 16px;
    border-radius: var(--l-radius);
    background: rgba(255, 255, 255, 0.03);
    /* Вторая, прозрачная тень снаружи расширяет область перерисовки на
       1 px: при масштабе экрана 125 % кольцо на скруглённых углах задевает
       соседний пиксель, и без неё после наведения там оставались точки. */
    box-shadow: inset 0 0 0 1px var(--l-border), 0 0 0 1px transparent;
    transition: box-shadow var(--l-fast), background-color var(--l-fast);

    &:not(.locked):hover {
      background-color: rgba(255, 255, 255, 0.05);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border)), 0 0 0 1px transparent;
    }

    &.secret:not(.locked) {
      background-image: radial-gradient(120% 140% at 0% 0%, rgba(219, 97, 162, 0.14) 0%, transparent 60%);
    }

    /* Редкость полученного (tier): золото и фиолет — оттенок фона и рамка,
       радуга (king-of-the-hill) — переливающаяся рамка. Стоят после
       секретного: золотое секретное остаётся золотым. */
    &.tier-gold { --tier: 245, 197, 66; }
    &.tier-purple { --tier: 168, 115, 255; }

    &.tier-gold:not(.locked),
    &.tier-purple:not(.locked) {
      background-image: radial-gradient(120% 140% at 0% 0%, rgba(var(--tier), 0.16) 0%, transparent 60%);
      box-shadow: inset 0 0 0 1px rgba(var(--tier), 0.5), 0 0 16px rgba(var(--tier), 0.14);

      &:hover { box-shadow: inset 0 0 0 1px rgba(var(--tier), 0.8), 0 0 18px rgba(var(--tier), 0.22); }
    }

    &.tier-rainbow:not(.locked) {
      position: relative;
      background-image: linear-gradient(115deg, rgba(255, 95, 109, 0.12), rgba(255, 195, 113, 0.1), rgba(95, 245, 154, 0.1), rgba(79, 195, 247, 0.1), rgba(167, 139, 250, 0.12));
      box-shadow: 0 0 18px rgba(167, 139, 250, 0.18), 0 0 0 1px transparent;

      /* Кольцо 1.5 px маской; перелив — сдвиг повторяющегося градиента ровно
         на одну его ширину (150 % при размере 300 %), без скачка. */
      &::before {
        content: "";
        position: absolute;
        inset: 0;
        padding: 1.5px;
        border-radius: inherit;
        pointer-events: none;
        background: linear-gradient(115deg, #ff5f6d, #ffc371, #f9f871, #5ff59a, #4fc3f7, #a78bfa, #ff5f6d) 0 0 / 300% 100%;
        -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0);
        -webkit-mask-composite: xor;
        mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0);
        animation: profile-rainbow 6s linear infinite;
      }
    }
  }

  &__achievement-content {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  &__badge {
    position: relative;
    width: 44px;
    height: 44px;
    border-radius: var(--l-radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 18px;
    background: var(--l-accent-soft);
    color: var(--l-accent);

    .secret:not(.locked) & {
      background: rgba(219, 97, 162, 0.16);
      color: #ec7fbb;
    }

    .tier-gold:not(.locked) &,
    .tier-purple:not(.locked) & {
      background: rgba(var(--tier), 0.18);
      color: rgb(var(--tier));
    }

    .tier-rainbow:not(.locked) & {
      background: linear-gradient(115deg, #ff5f6d, #ffc371, #5ff59a, #4fc3f7, #a78bfa, #ff5f6d) 0 0 / 300% 100%;
      color: #fff;
      text-shadow: 0 1px 2px rgba(0, 0, 0, 0.35);
      animation: profile-rainbow 6s linear infinite;
    }

    .locked & {
      background: rgba(255, 255, 255, 0.05);
      color: var(--l-text-faint);
    }
  }

  &__lock {
    position: absolute;
    right: -4px;
    bottom: -4px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 9px;
    background: var(--l-modal-bg);
    border: 1px solid var(--l-border);
    color: var(--l-text-faint);
  }

  &__achievement-title {
    color: var(--l-text);
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;

    .locked & { color: var(--l-text-dim); }
  }

  &__achievement-text {
    color: var(--l-text-dim);
    font-size: 12.5px;
    line-height: 18px;
  }

  &__state {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--l-text-faint);
    font-size: 11.5px;
    font-weight: 600;
    line-height: 20px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;

    &.is-done { color: #4ad07a; }
  }

  &__done {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
  }

  &__done-at {
    color: var(--l-text-faint);
    font-size: 11px;
    line-height: 16px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  &__achievement-meter {
    height: 4px;
    margin-top: 8px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }

  &__achievement-meter-fill {
    height: 100%;
    background: var(--l-accent);
    transition: width var(--l-slow);
  }

}

/* Низкое окно (Full HD): шапка компактнее — аватар меньше, значки и дата в одну
   строку под ником, вкладки ниже, — и разделам достаётся на ~100 px больше. */
@media (max-height: 700px) {
  .profile__close { top: 16px; }
  .profile__banner { height: 66px; }
  .profile__identity { padding-top: 24px; }

  .profile__avatar {
    flex-basis: 72px;
    width: 72px;
    height: 72px;
    font-size: 28px;
  }

  .profile__who {
    flex-direction: row;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
    padding-top: 6px;
  }

  .profile__name { flex: 1 1 100%; font-size: 22px; line-height: 30px; }
  .profile__badges { margin: 0; }
  .runner-status { order: 1; flex: 1 1 100%; }
  .profile__tabs { margin-top: 8px; }
  .profile__tab { height: 40px; }
}

@media (max-width: 860px) {
  .profile__overview,
  .profile__achievements {
    grid-template-columns: minmax(0, 1fr);
  }

  .profile__overview {
    grid-template-rows: none;
    grid-auto-flow: row;
  }

  /* одна колонка: явное место детей снимается, всё идёт подряд */
  .profile__overview .profile__column:nth-child(n) > :nth-child(n) {
    grid-column: auto;
    grid-row: auto;
  }
}

/* Человек попросил систему не двигать ничего лишнего — уважаем */
@media (prefers-reduced-motion: reduce) {
  .profile__meter-fill,
  .profile__tab-ink,
  .runner-record,
  .profile__achievement {
    transition: none;
  }
}

/* Тема Liquid Glass задаёт окнам свою тень (launcher.css): рамка подписчика
   ложится поверх неё, блик стекла остаётся. */
.launcher-root[data-theme="liquid-glass"] .profile__window.is-subscriber {
  box-shadow: $subscriber-ring, var(--lg-rim), var(--lg-shadow);
}
.launcher-root[data-theme="liquid-glass"] .profile__window.is-complete {
  box-shadow: $complete-ring, var(--lg-rim), var(--lg-shadow);
}

/* Радужное достижение переливается; без анимаций (система или
   переключатель лаунчера) — стоит. */
@keyframes profile-rainbow {
  to { background-position: 150% 0; }
}
@media (prefers-reduced-motion: reduce) {
  .profile__achievement.tier-rainbow::before,
  .profile__achievement.tier-rainbow .profile__badge { animation: none; }
}
.launcher-root.no-anim .profile__achievement.tier-rainbow::before,
.launcher-root.no-anim .profile__achievement.tier-rainbow .profile__badge { animation: none !important; }

/* Меню «⋯» в шапке: поверхность и пункты — как у меню игры (GameMenu.vue),
   открывается вниз. */
.profile-more { position: relative; display: inline-flex; }
.profile-more__trigger {
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border: 0;
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.04);
  color: var(--l-text-dim);
  font-size: 13px;
  cursor: pointer;
  transition: background var(--l-fast), color var(--l-fast);
  &:hover, &.open { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
  &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
}
.profile-more__menu {
  position: absolute;
  top: calc(100% + 8px);
  left: 0;
  z-index: 5;
  width: 248px;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  background: var(--l-modal-bg);
  border-radius: var(--l-radius);
  box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
  transform-origin: 14px 0;
}
.profile-more__item {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  padding: 8px;
  border: none;
  border-radius: var(--l-radius-sm);
  background: transparent;
  box-shadow: inset 0 0 0 1px transparent;
  color: var(--l-text);
  font-family: inherit;
  font-size: 14px;
  line-height: 20px;
  text-align: left;
  white-space: nowrap;
  cursor: pointer;
  transition: background-color var(--l-fast), box-shadow var(--l-fast);
  animation: l-rise-in var(--l-dur) var(--l-ease) both;
  animation-delay: calc(var(--i, 0) * 24ms);
  &:hover { background: rgba(255, 255, 255, 0.05); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border)); }
  &:active { background: rgba(255, 255, 255, 0.08); }
  &:focus-visible { outline: none; background: rgba(255, 255, 255, 0.05); box-shadow: inset 0 0 0 1px var(--l-accent); }
}
.profile-more__icon {
  flex: none;
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.04);
  box-shadow: inset 0 0 0 1px var(--l-border);
  color: var(--l-text-dim);
  font-size: 13px;
  transition: background-color var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);
  > i { line-height: 1; }
}
.profile-more__item:hover .profile-more__icon,
.profile-more__item:focus-visible .profile-more__icon { background: var(--l-accent-soft); box-shadow: inset 0 0 0 1px transparent; color: var(--l-accent); }
.profile-more-enter-active { transition: opacity var(--l-dur) var(--l-ease), transform var(--l-dur) var(--l-ease); }
.profile-more-leave-active { transition: opacity var(--l-dur-fast) var(--l-ease-in), transform var(--l-dur-fast) var(--l-ease-in); }
.profile-more-enter-from, .profile-more-leave-to { opacity: 0; transform: translateY(calc(-1 * var(--l-rise))) scale(0.96); }
.no-anim .profile-more-enter-active, .no-anim .profile-more-leave-active { transition: none; }
@media (prefers-reduced-motion: reduce) {
  .profile-more-enter-active, .profile-more-leave-active { transition: none; }
  .profile-more__item { animation: none; }
}

/* Рейтинг профиля в строке под ником. */
.profile-rating {
  display: inline-flex;
  /* Пальцы и счёт — на общей базовой линии, как в строке «В HLSR с …». */
  align-items: baseline;
  gap: 2px;
  padding: 2px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.06);
  box-shadow: inset 0 0 0 1px var(--l-border);
  color: var(--l-text-dim);
  font-size: 12.5px;
  font-variant-numeric: tabular-nums;
}
/* Звезда — в такой же ячейке, как кнопки голоса: пилюля той же высоты.
   Глиф шрифта садится на базовую линию, и её округление до пикселя
   на 100 % опускало звезду, а на 125 % поднимало. Поэтому звезда —
   маска по контуру fa-star (viewBox по её рамке), центр ячейки точный
   на любом масштабе. Счёт рядом выровнен по центру пилюли. */
.profile-rating__icon { display: grid; place-items: center; width: 26px; height: 26px; }
.profile-rating__icon::before {
  content: "";
  width: 12px;
  height: 11.5px;
  background: currentColor;
  -webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='12 -32 552 526'%3E%3Cpath d='M309.5-18.9c-4.1-8-12.4-13.1-21.4-13.1s-17.3 5.1-21.4 13.1L193.1 125.3 33.2 150.7c-8.9 1.4-16.3 7.7-19.1 16.3s-.5 18 5.8 24.4l114.4 114.5-25.2 159.9c-1.4 8.9 2.3 17.9 9.6 23.2s16.9 6.1 25 2L288.1 417.6 432.4 491c8 4.1 17.7 3.3 25-2s11-14.2 9.6-23.2L441.7 305.9 556.1 191.4c6.4-6.4 8.6-15.8 5.8-24.4s-10.1-14.9-19.1-16.3L383 125.3 309.5-18.9z'/%3E%3C/svg%3E") center / contain no-repeat;
  mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='12 -32 552 526'%3E%3Cpath d='M309.5-18.9c-4.1-8-12.4-13.1-21.4-13.1s-17.3 5.1-21.4 13.1L193.1 125.3 33.2 150.7c-8.9 1.4-16.3 7.7-19.1 16.3s-.5 18 5.8 24.4l114.4 114.5-25.2 159.9c-1.4 8.9 2.3 17.9 9.6 23.2s16.9 6.1 25 2L288.1 417.6 432.4 491c8 4.1 17.7 3.3 25-2s11-14.2 9.6-23.2L441.7 305.9 556.1 191.4c6.4-6.4 8.6-15.8 5.8-24.4s-10.1-14.9-19.1-16.3L383 125.3 309.5-18.9z'/%3E%3C/svg%3E") center / contain no-repeat;
}
.profile-rating.is-static { align-items: center; }
/* Пальцы и счёт сдвинуты вместе: относительно друг друга они стоят на общей
   базовой линии, а целиком в ячейке 26 px глиф FA садится выше середины. */
.profile-rating__btn > i,
.profile-rating__score { transform: translateY(1px); }
.profile-rating__score { min-width: 26px; padding: 0 4px; font-weight: 600; text-align: center; }
.profile-rating.is-static .profile-rating__score { min-width: 0; padding: 0 10px 0 0; transform: none; }
.profile-rating.is-positive .profile-rating__score { color: #5fd38a; }
.profile-rating.is-negative .profile-rating__score { color: #ff8a96; }
.profile-rating__btn {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: var(--l-text-dim);
  font-size: 12px;
  cursor: pointer;
  transition: background var(--l-fast), color var(--l-fast), transform 90ms ease-out;
  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); color: var(--l-text); }
  &:active:not(:disabled) { transform: scale(0.9); }
  &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 1px; }
  &:disabled { cursor: progress; opacity: 0.6; }
  &.is-like.is-on { background: rgba(95, 211, 138, 0.18); color: #5fd38a; }
  &.is-dislike.is-on { background: rgba(255, 138, 150, 0.18); color: #ff8a96; }
}
.profile-rating__dubious {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 9px;
  border-radius: 999px;
  background: rgba(255, 176, 32, 0.14);
  box-shadow: inset 0 0 0 1px rgba(255, 176, 32, 0.4);
  color: #ffc35a;
  font-size: 12px;
  font-weight: 600;
}
.profile-rating-anchor { position: relative; display: inline-flex; }
.profile-rating__note {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  z-index: 3;
  padding: 5px 10px;
  border-radius: var(--l-radius-sm);
  background: var(--l-bg-raised);
  box-shadow: inset 0 0 0 1px var(--l-border), 0 8px 24px rgba(0, 0, 0, 0.45);
  color: var(--l-text-dim);
  font-size: 12px;
  line-height: 16px;
  white-space: nowrap;
  pointer-events: none;
}
</style>
