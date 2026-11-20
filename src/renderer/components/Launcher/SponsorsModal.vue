<template>
  <div class="settings-modal" @mousedown.self="$emit('close')">
    <div
      class="settings-modal__window sponsors-modal"
      role="dialog"
      aria-modal="true"
      :aria-label="$t(legacy ? '#L_S_SPONSORS' : '#L_SPONSORS_TITLE')"
      ref="window"
    >
      <!-- Бегущая розовая рамка окна (neon.scss). -->
      <span class="neon" aria-hidden="true"></span>
      <button
        type="button"
        class="sponsors-modal__close"
        :title="$t('#L_CLOSE')"
        :aria-label="$t('#L_CLOSE')"
        @click="$emit('close')"
      >
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>

      <!-- Пока нет ответа, неизвестно, какое окно рисовать: только загрузка,
           чтобы при старом сервере не мелькнула шапка нового окна. -->
      <div v-if="view === 'loading'" class="sponsors-modal__loading">
        <i class="fas fa-spinner fa-spin" aria-hidden="true"></i>
        <span>{{ $t("#L_SPONSORS_LOADING") }}</span>
      </div>

      <!-- Старый сервер (PHP, только /v1): окно как в 0.4.7, без кнопок и разделов. -->
      <template v-else-if="view === 'legacy'">
        <!-- Баннер в розовом GitHub Sponsors, как сердечко в шапке. -->
        <header class="sponsors-modal__hero">
          <span class="sponsors-modal__mark" aria-hidden="true">
            <i class="fas fa-heart"></i>
          </span>
          <div class="sponsors-modal__intro">
            <h2 class="sponsors-modal__title">{{ $t("#L_S_SPONSORS") }}</h2>
            <p class="sponsors-modal__lead">{{ $t("#L_SPONSORS_LEAD") }}</p>
          </div>
        </header>

        <div class="sponsors-modal__body">
          <h3 class="sponsors-modal__section">{{ $t("#L_SPONSORS_LIST") }}</h3>

          <!-- Царь горы: кто дал больше всех, тот наверху и крупнее всех.
               Ниже — 2-е и 3-е места, дальше компактная сетка. -->
          <ol v-if="sponsors.length" class="sponsors-modal__hill">
            <li v-for="(sponsor, index) in sponsors" :key="sponsor.id" class="sponsors-modal__item" :class="'is-' + tier(index)">
              <component
                :is="sponsor.profile ? 'button' : 'div'"
                :type="sponsor.profile ? 'button' : null"
                class="sponsors-modal__card"
                :class="{ 'is-link': sponsor.profile }"
                :title="sponsor.profile ? $t('#L_SPONSORS_OPEN_PROFILE') : null"
                @click="sponsor.profile && $emit('open-profile', sponsor.profile.steamId64)"
              >
                <span class="sponsors-modal__seat">
                  <AvatarFrame :frame="sponsor.profile && sponsor.profile.avatarFrame" :width="2" hole="var(--l-modal-bg)">
                    <span class="sponsors-modal__avatar" aria-hidden="true">
                      <b>{{ initial(sponsor.name) }}</b>
                      <img v-if="avatar(sponsor, sponsor.id)" :src="avatar(sponsor, sponsor.id)" alt="" referrerpolicy="no-referrer" @error="brokenAvatars[sponsor.id] = true" />
                    </span>
                  </AvatarFrame>
                  <i v-if="index === 0" class="fas fa-crown sponsors-modal__crown" aria-hidden="true"></i>
                </span>
                <span class="sponsors-modal__text">
                  <span v-if="index === 0" class="sponsors-modal__title-king">{{ $t("#L_SPONSORS_KING") }}</span>
                  <NickName class="sponsors-modal__name" :nick="sponsor.profile && sponsor.profile.nick" :subscriber="!!sponsor.profile && sponsor.profile.subscriber === true" :title="sponsor.name">{{ sponsor.name }}</NickName>
                  <span class="sponsors-modal__amount" :title="amountText(sponsor.amount)">{{ amountShort(sponsor.amount) }}</span>
                  <span v-if="sponsor.note" class="sponsors-modal__note" :title="sponsor.note">{{ sponsor.note }}</span>
                </span>
                <span v-if="index > 0" class="sponsors-modal__place" :aria-label="$t('#L_SPONSORS_PLACE', index + 1)">{{ index + 1 }}</span>
              </component>
              <p v-if="index === 0" class="sponsors-modal__dethrone">{{ $t("#L_SPONSORS_DETHRONE", amountShort(sponsor.amount)) }}</p>
            </li>
          </ol>

          <div v-else class="sponsors-modal__empty">
            <i class="far fa-heart" aria-hidden="true"></i>
            <span>{{ $t(failed ? "#L_SPONSORS_FAILED" : "#L_S_SPONSORS_EMPTY") }}</span>
          </div>
        </div>
      </template>

      <!-- Новое окно: пояснение и кнопки Tribute сверху, ниже три раздела по
           центру — подписчики, донаты («царь горы»), поддерживали раньше. -->
      <template v-else>
      <div class="sponsors-modal__scroll">
        <header class="sponsors-modal__head">
          <span class="sponsors-modal__badge" aria-hidden="true">
            <i class="fas fa-heart"></i>
          </span>
          <h2 class="sponsors-modal__heading">{{ $t("#L_SPONSORS_TITLE") }}</h2>
          <p class="sponsors-modal__about">{{ $t("#L_SPONSORS_ABOUT") }}</p>

          <template v-if="supportKeys.length">
            <div class="sponsors-modal__support">
              <button
                v-for="key in supportKeys"
                :key="key"
                type="button"
                class="sponsors-modal__cta"
                :class="'is-' + key"
                @click="support(key)"
              >
                <i :class="SUPPORT[key].icon" aria-hidden="true"></i>
                <span>{{ $t(SUPPORT[key].label) }}</span>
              </button>
            </div>
          </template>
        </header>

        <div v-if="!sections.length" class="sponsors-modal__state">
          <i class="far fa-heart" aria-hidden="true"></i>
          <span>{{ $t(failed ? "#L_SPONSORS_FAILED" : "#L_S_SPONSORS_EMPTY") }}</span>
        </div>

        <template v-else>
          <section v-for="section in sections" :key="section.key" class="sponsors-modal__part" :class="'is-' + section.key">
            <h3 class="sponsors-modal__label">
              <i :class="PARTS[section.key].icon" aria-hidden="true"></i>
              <span>{{ $t(PARTS[section.key].label) }}</span>
            </h3>

            <!-- Подписчики: аватар и ник в своей подсветке и рамке. -->
            <ul v-if="section.key === 'active'" class="sponsors-modal__people">
              <li v-for="(entry, index) in section.entries" :key="index">
                <component
                  :is="action(entry) ? 'button' : 'div'"
                  :type="action(entry) ? 'button' : null"
                  class="sponsors-modal__person"
                  :class="{ 'is-link': action(entry) }"
                  :title="linkTitle(entry)"
                  @click="activate(section.key, index)"
                >
                  <AvatarFrame :frame="entry.profile && entry.profile.avatarFrame" :width="2" hole="var(--l-modal-bg)">
                    <span class="sponsors-modal__avatar" aria-hidden="true">
                      <b>{{ initial(entryName(entry)) }}</b>
                      <img v-if="avatar(entry, 'active' + index)" :src="avatar(entry, 'active' + index)" alt="" referrerpolicy="no-referrer" @error="brokenAvatars['active' + index] = true" />
                    </span>
                  </AvatarFrame>
                  <NickName
                    class="sponsors-modal__nick"
                    :class="{ 'is-anonymous': !entryName(entry) }"
                    :nick="entryName(entry) && entry.profile ? entry.profile.nick : null"
                    :subscriber="subscriberOf(entry)"
                  >{{ nameOf(entry) }}</NickName>
                </component>
              </li>
            </ul>

            <!-- Донаты, «царь горы»: первый крупно с короной, дальше по сумме. -->
            <ol v-else-if="section.key === 'donors'" class="sponsors-modal__donors">
              <li v-for="(entry, index) in section.entries" :key="index" class="sponsors-modal__donor" :class="'is-' + tier(index)">
                <component
                  :is="action(entry) ? 'button' : 'div'"
                  :type="action(entry) ? 'button' : null"
                  class="sponsors-modal__plate"
                  :class="{ 'is-link': action(entry) }"
                  :title="linkTitle(entry)"
                  @click="activate(section.key, index)"
                >
                  <span class="sponsors-modal__seat">
                    <AvatarFrame :frame="entry.profile && entry.profile.avatarFrame" :width="2" hole="var(--l-modal-bg)">
                      <span class="sponsors-modal__avatar" aria-hidden="true">
                        <b>{{ initial(entryName(entry)) }}</b>
                        <img v-if="avatar(entry, 'donors' + index)" :src="avatar(entry, 'donors' + index)" alt="" referrerpolicy="no-referrer" @error="brokenAvatars['donors' + index] = true" />
                      </span>
                    </AvatarFrame>
                    <i v-if="index === 0" class="fas fa-crown sponsors-modal__crown" aria-hidden="true"></i>
                  </span>
                  <span class="sponsors-modal__who">
                    <NickName
                      class="sponsors-modal__nick"
                      :class="{ 'is-anonymous': !entryName(entry) }"
                      :nick="entryName(entry) && entry.profile ? entry.profile.nick : null"
                      :subscriber="subscriberOf(entry)"
                      :title="nameOf(entry)"
                    >{{ nameOf(entry) }}</NickName>
                    <span class="sponsors-modal__sum" :title="amountText(entry.amount)">{{ amountShort(entry.amount) }}</span>
                    <span v-if="entry.note" class="sponsors-modal__words" :title="entry.note">{{ entry.note }}</span>
                  </span>
                  <span v-if="index > 0" class="sponsors-modal__rank" :aria-label="$t('#L_SPONSORS_PLACE', index + 1)">{{ index + 1 }}</span>
                </component>
                <p v-if="index === 0" class="sponsors-modal__throne">{{ $t("#L_SPONSORS_DETHRONE", amountShort(entry.amount)) }}</p>
              </li>
            </ol>

            <!-- Поддерживали раньше: обычные ники, без подсветки. -->
            <ul v-else class="sponsors-modal__names">
              <li v-for="(entry, index) in section.entries" :key="index">
                <component
                  :is="action(entry) ? 'button' : 'span'"
                  :type="action(entry) ? 'button' : null"
                  class="sponsors-modal__plain"
                  :class="{ 'is-link': action(entry), 'is-anonymous': !entryName(entry) }"
                  :title="linkTitle(entry) || nameOf(entry)"
                  @click="activate(section.key, index)"
                >{{ nameOf(entry) }}</component>
              </li>
            </ul>
          </section>
        </template>
      </div>
      <p v-if="supportKeys.length" class="sponsors-modal__hint">{{ $t("#L_SPONSORS_HINT") }}</p>
      </template>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";
import uiState from "./ui-state";
import NickName from "./NickName";
import AvatarFrame from "./AvatarFrame";

const sponsorSections = require("../../../shared/sponsor-sections");

const { ipcRenderer } = window.hlsrElectron;

// Кнопки оплаты: иконка повторяет раздел, куда они ведут (звезда —
// подписчики, корона — донаты). Обе открывают нашего Telegram-бота (main,
// support-open): бот сначала привязывает Telegram к Steam, потом даёт оплату.
const SUPPORT = {
  subscribe: { icon: "fas fa-star", label: "#L_SPONSORS_SUBSCRIBE" },
  donate: { icon: "fas fa-crown", label: "#L_SPONSORS_DONATE" },
};

const PARTS = {
  active: { icon: "fas fa-star", label: "#L_SPONSORS_ACTIVE" },
  donors: { icon: "fas fa-crown", label: "#L_SPONSORS_DONORS" },
  past: { icon: "fas fa-heart", label: "#L_SPONSORS_PAST" },
};

// Список запрашивается один раз, при открытии окна: sponsor-open-link
// открывает запись по номеру в последнем списке main, и окно не должно
// разойтись с ним, пока открыто.
export default {
  name: "SponsorsModal",
  components: { NickName, AvatarFrame },
  emits: ["close", "open-profile"],
  data() {
    return { result: null, loading: true, brokenAvatars: {}, SUPPORT, PARTS };
  },
  computed: {
    // Что показывает окно: "loading" — ждём ответа, "legacy" — окно 0.4.7,
    // "window" — новое окно (в том числе с ошибкой или пустым списком).
    view() {
      if (this.loading) return "loading";
      return this.legacy ? "legacy" : "window";
    },
    legacy() {
      return sponsorSections.isLegacy(this.result);
    },
    sections() {
      return this.legacy ? [] : sponsorSections.sectionsOf(this.result);
    },
    // Список 0.4.7 (старый сервер): { id, name, note, amount, profile }.
    sponsors() {
      return this.legacy ? sponsorSections.sectionsOf(this.result)[0].entries : [];
    },
    supportKeys() {
      return this.legacy || !this.result ? [] : sponsorSections.supportKeys(this.result.links);
    },
    failed() {
      return !this.loading && (!this.result || this.result.status !== "ok");
    },
  },
  async created() {
    this.result = await ipcRenderer.invoke("public-sponsors-get").catch(() => null);
    this.loading = false;
  },
  methods: {
    tier(index) {
      return sponsorSections.sponsorTier(index);
    },
    numberLocale() {
      return Object.keys(this.$localization.locales)[this.$localization.lang] === "ru" ? "ru-RU" : "en-GB";
    },
    amountText(amount) {
      return sponsorSections.amountText(amount, this.numberLocale());
    },
    amountShort(amount) {
      return sponsorSections.amountShort(amount, this.numberLocale());
    },
    entryName(entry) {
      return sponsorSections.entryName(entry);
    },
    nameOf(entry) {
      return sponsorSections.entryName(entry) || this.$t("#L_SPONSORS_ANONYMOUS");
    },
    subscriberOf(entry) {
      return sponsorSections.entrySubscriber(entry);
    },
    action(entry) {
      return sponsorSections.entryAction(entry);
    },
    linkTitle(entry) {
      const action = sponsorSections.entryAction(entry);
      if (action === "profile") return this.$t("#L_SPONSORS_OPEN_PROFILE");
      if (action !== "url") return null;
      try {
        return this.$t("#L_SPONSORS_OPEN_LINK", new URL(entry.link.url).hostname);
      } catch (error) {
        return null;
      }
    },
    // Клик по нику: профиль HLSR (по link.steamId64) или свой адрес — его
    // main берёт из своего списка по разделу и номеру и открывает после
    // подтверждения. Остальное не кликается.
    activate(section, index) {
      if (this.legacy || !this.result || !sponsorSections.SECTIONS.includes(section) || !Array.isArray(this.result[section])) return;
      const entry = this.result[section][index];
      const action = sponsorSections.entryAction(entry);
      if (action === "profile") this.$emit("open-profile", entry.link.steamId64);
      else if (action === "url") ipcRenderer.invoke("sponsor-open-link", { section, index }).catch(() => null);
    },
    // Кнопки Tribute и бота: адрес знает только main.
    support(key) {
      if (this.supportKeys.includes(key)) ipcRenderer.invoke("support-open", key).catch(() => null);
    },
    avatar(entry, key) {
      return !this.brokenAvatars[key] && entry.profile && entry.profile.avatarUrl ? entry.profile.avatarUrl : null;
    },
    initial(name) {
      const text = String(name || "?").trim();
      return text ? Array.from(text)[0].toUpperCase() : "?";
    },
    onKeydown(e) {
      if (e.key === "Escape") {
        if (uiState.openDropdowns > 0) return;
        e.preventDefault();
        this.$emit("close");
      } else if (e.key === "Tab") {
        trapFocus(this.$refs.window, e);
      }
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);
    this.$nextTick(() => {
      const btn = this.$refs.window.querySelector("button");
      if (btn) btn.focus();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.isConnected) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
@use "./neon" as *;
@include neon(sponsors-neon, #ff4fd8, #ff9de6, rgba(255, 79, 216, 0.2));

.settings-modal {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;

  &__window {
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }
}

/* Розовый GitHub Sponsors (#db61a2) — цвет сердечка в шапке; золото —
   корона «царя горы». */
.sponsors-modal {
  --sponsor: #db61a2;
  --gold: #ffd166;

  position: relative;
  box-shadow: inset 0 0 0 1px var(--l-modal-border), inset 0 0 16px rgba(255, 79, 216, 0.2), 0 0 28px rgba(255, 79, 216, 0.28), 0 24px 72px rgba(0, 0, 0, 0.6);
  width: min(980px, calc(100% - 80px));
  height: min(700px, calc(100% - 80px));

  &__close {
    position: absolute;
    top: 24px;
    right: 24px;
    z-index: 1;
    width: 40px;
    height: 40px;
    padding: 0;
    display: grid;
    place-items: center;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(0, 0, 0, 0.32);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 15px;
    cursor: pointer;
    transition: background-color var(--l-fast), color var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.1);
      color: var(--l-text);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }
  }

  // Загрузка: одно сообщение посреди окна, без шапки.
  &__loading {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: var(--l-text-dim);
    font-size: 14px;
    line-height: 20px;

    i {
      color: var(--sponsor);
      font-size: 24px;
    }
  }

  // ── Новое окно ──────────────────────────────────────────────────────────

  // Прокручивается всё окно целиком: шапка с пояснением высокая, и на
  // небольшом окне лаунчера разделам иначе осталось бы мало места.
  &__scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 48px 12px;
  }

  // Розовое свечение сверху по центру — продолжение сердечка в шапке.
  &__head {
    display: flex;
    flex-direction: column;
    align-items: center;
    margin: 0 -48px;
    padding: 40px 112px 32px;
    background: radial-gradient(46% 240px at 50% 0%, color-mix(in srgb, var(--sponsor) 20%, transparent) 0%, transparent 100%);
    text-align: center;
  }

  &__badge {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--sponsor) 16%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 40%, transparent), 0 0 32px color-mix(in srgb, var(--sponsor) 24%, transparent);
    color: var(--sponsor);
    font-size: 20px;
  }

  &__heading {
    margin: 14px 0 0;
    color: var(--l-text);
    font-size: 26px;
    font-weight: 700;
    line-height: 32px;
  }

  &__about {
    max-width: 620px;
    margin: 10px 0 0;
    color: var(--l-text-dim);
    font-size: 14px;
    line-height: 22px;
    text-wrap: pretty;
  }

  &__support {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
    margin-top: 24px;
  }

  // Кнопки как в остальных окнах лаунчера; главная — подписка, розовая.
  &__cta {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    height: 40px;
    padding: 0 18px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
    transition: background-color var(--l-fast), filter var(--l-fast), transform var(--l-fast);

    i {
      font-size: 13px;
    }

    &:hover {
      background: rgba(255, 255, 255, 0.1);
    }

    &:active {
      transform: scale(0.97);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }

    &.is-subscribe {
      border-color: transparent;
      background: var(--sponsor);
      box-shadow: 0 6px 22px color-mix(in srgb, var(--sponsor) 32%, transparent);
      color: #fff;

      &:hover {
        background: var(--sponsor);
        filter: brightness(1.08);
      }
    }

    &.is-donate i {
      color: var(--gold);
    }
  }

  // Серая строка внизу окна, вне прокрутки.
  &__hint {
    flex: none;
    margin: 0;
    padding: 8px 48px 12px;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 17px;
    text-align: center;
  }

  &__state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    min-height: 160px;
    padding: 24px;
    border-radius: var(--l-radius);
    background: rgba(255, 255, 255, 0.02);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 14px;
    line-height: 20px;
    text-align: center;

    i {
      color: color-mix(in srgb, var(--sponsor) 70%, var(--l-text-faint));
      font-size: 24px;
    }
  }

  // Раздел: подпись с иконкой по центру, под ней ники; сверху — тонкий
  // разделитель, гаснущий к краям.
  &__part {
    --tone: var(--sponsor);
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 28px 0 32px;
    text-align: center;

    &::before {
      content: "";
      position: absolute;
      top: 0;
      left: 6%;
      right: 6%;
      height: 1px;
      background: linear-gradient(90deg, transparent, var(--l-modal-border) 22%, var(--l-modal-border) 78%, transparent);
    }

    &.is-donors {
      --tone: var(--gold);
    }

    &.is-past {
      --tone: color-mix(in srgb, var(--sponsor) 55%, var(--l-text-faint));
    }
  }

  &__label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 20px;
    color: color-mix(in srgb, var(--tone) 45%, var(--l-text-dim));
    font-size: 11.5px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.14em;
    text-transform: uppercase;

    i {
      color: var(--tone);
      font-size: 12px;
      filter: drop-shadow(0 0 6px color-mix(in srgb, var(--tone) 45%, transparent));
    }
  }

  /* Ник с подсветкой: её свечение выходит за буквы, а overflow ради
     многоточия срезал бы его — поля под свечение съедаются отрицательным
     отступом, раскладка не меняется (как у ника в профиле). */
  &__nick {
    min-width: 0;
    margin: -8px -14px;
    padding: 8px 14px;
    overflow: hidden;
    color: var(--l-text);
    font-size: var(--nick, 15px);
    font-weight: 700;
    line-height: 1.3;
    text-overflow: ellipsis;
    white-space: nowrap;

    &.is-anonymous {
      color: var(--l-text-faint);
      font-style: italic;
      font-weight: 600;
    }
  }

  // Подписчики: аватар и ник, без плашки — подсветка ника и есть украшение.
  &__people {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  &__person {
    --avatar: 34px;
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 280px;
    margin: 0;
    padding: 6px 16px 6px 6px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: inherit;
    font: inherit;

    &.is-link {
      cursor: pointer;
      transition: background-color var(--l-fast);

      &:hover {
        background: color-mix(in srgb, var(--sponsor) 10%, transparent);
      }

      &:focus-visible {
        outline: 2px solid var(--l-accent);
        outline-offset: 2px;
      }
    }
  }

  // Донаты: трон на всю ширину, пьедестал (2-е и 3-е) — два в ряд, дальше
  // по три в ряд; ряды по центру и одной ширины (2 × 384 = 3 × 252 + зазоры).
  // Пьедестал всегда в своём ряду: ::after с order 2 — разрыв строки между
  // ним и остальными, порядок в разметке (и для экранного диктора) прежний.
  &__donors {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
    width: 100%;
    max-width: 800px;
    margin: 0;
    padding: 0;
    list-style: none;

    &::after {
      content: "";
      order: 2;
      flex-basis: 100%;
      height: 0;
    }
  }

  &__donor {
    --avatar: 34px;
    --nick: 14px;
    --place: var(--l-text-faint);
    flex: 0 1 252px;
    order: 3;
    min-width: 0;
  }

  &__donor.is-podium {
    --avatar: 42px;
    --nick: 16px;
    flex-basis: 384px;
    order: 1;
  }

  &__donor.is-podium:nth-child(2) { --place: #cfd8e3; }
  &__donor.is-podium:nth-child(3) { --place: #e0a370; }

  &__donor.is-king {
    --avatar: 76px;
    --nick: 26px;
    display: flex;
    flex: 0 0 100%;
    flex-direction: column;
    align-items: center;
    order: 0;
    margin-bottom: 8px;
  }

  &__plate {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    height: 100%;
    min-width: 0;
    margin: 0;
    padding: 10px 12px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: inherit;
    font: inherit;
    text-align: left;

    &.is-link {
      cursor: pointer;
      transition: background-color var(--l-fast), box-shadow var(--l-fast);

      &:hover {
        background: color-mix(in srgb, var(--sponsor) 8%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 40%, transparent);
      }

      &:focus-visible {
        outline: 2px solid var(--l-accent);
        outline-offset: 2px;
      }
    }
  }

  .is-podium &__plate {
    gap: 14px;
    padding: 12px 14px;
  }

  // Трон: столбиком по центру, золотая подсветка, корона над аватаром.
  .is-king &__plate {
    flex-direction: column;
    gap: 14px;
    width: min(520px, 100%);
    height: auto;
    padding: 34px 28px 22px;
    background:
      radial-gradient(70% 120% at 50% 0%, color-mix(in srgb, var(--gold) 16%, transparent) 0%, transparent 70%),
      color-mix(in srgb, var(--gold) 4%, rgba(255, 255, 255, 0.03));
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--gold) 45%, transparent), 0 8px 32px color-mix(in srgb, var(--gold) 10%, transparent);
    text-align: center;

    &.is-link:hover {
      background:
        radial-gradient(70% 120% at 50% 0%, color-mix(in srgb, var(--gold) 22%, transparent) 0%, transparent 70%),
        color-mix(in srgb, var(--gold) 6%, rgba(255, 255, 255, 0.03));
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--gold) 70%, transparent), 0 8px 32px color-mix(in srgb, var(--gold) 16%, transparent);
    }
  }

  &__who {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .is-king &__who {
    align-items: center;
    width: 100%;
  }

  .is-king &__nick {
    max-width: 100%;
  }

  &__sum {
    overflow: hidden;
    color: var(--l-text-dim);
    font-size: 12px;
    font-weight: 600;
    line-height: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .is-king &__sum {
    margin-top: 4px;
    color: var(--gold);
    font-size: 16px;
    line-height: 22px;
  }

  /* Сообщение — до двух строк (у трона до трёх), длинные слова и ссылки
     переносятся; целиком — в подсказке. */
  &__words {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    overflow: hidden;
    margin-top: 2px;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
    overflow-wrap: anywhere;
  }

  .is-king &__words {
    -webkit-line-clamp: 3;
    max-width: 440px;
    margin-top: 8px;
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 19px;
  }

  // Номер места: серебро и бронза у пьедестала, серый у остальных.
  &__rank {
    flex: none;
    align-self: flex-start;
    min-width: 22px;
    padding: 0 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--place) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--place) 40%, transparent);
    color: var(--place);
    font-size: 11px;
    font-weight: 700;
    line-height: 20px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  &__throne {
    margin: 10px 0 0;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
  }

  // Поддерживали раньше: обычный текст, мельче.
  &__names {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px 22px;
    max-width: 760px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  &__plain {
    display: block;
    max-width: 220px;
    margin: 0;
    padding: 2px 0;
    overflow: hidden;
    border: none;
    background: none;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 13.5px;
    font-weight: 600;
    line-height: 20px;
    text-overflow: ellipsis;
    white-space: nowrap;

    &.is-link {
      cursor: pointer;
      transition: color var(--l-fast);

      &:hover {
        color: var(--l-text);
      }

      &:focus-visible {
        outline: 2px solid var(--l-accent);
        outline-offset: 2px;
        border-radius: 4px;
      }
    }

    &.is-anonymous {
      color: var(--l-text-faint);
      font-style: italic;
    }
  }

  // Общее: аватар-заглушка с буквой, корона трона.
  &__seat {
    position: relative;
    flex: none;
  }

  &__crown {
    position: absolute;
    top: -22px;
    left: 50%;
    color: var(--gold);
    font-size: 20px;
    filter: drop-shadow(0 2px 6px color-mix(in srgb, var(--gold) 50%, transparent));
    transform: translateX(-50%) rotate(-12deg);
  }

  .is-king &__crown {
    top: -27px;
    font-size: 24px;
  }

  &__avatar {
    position: relative;
    flex: none;
    display: grid;
    place-items: center;
    width: var(--avatar);
    height: var(--avatar);
    overflow: hidden;
    border-radius: 50%;
    background: color-mix(in srgb, var(--sponsor) 18%, transparent);
    color: var(--sponsor);
    font-size: calc(var(--avatar) * 0.44);
    font-weight: 700;

    img {
      position: absolute;
      inset: 0;
      width: 100%;
      height: 100%;
      object-fit: cover;
    }
  }

  .is-king &__avatar {
    background: color-mix(in srgb, var(--gold) 16%, transparent);
    color: var(--gold);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--gold) 70%, transparent);
  }

  // ── Окно 0.4.7 (старый сервер) ──────────────────────────────────────────

  &__hero {
    flex: none;
    display: flex;
    align-items: center;
    gap: 24px;
    padding: 40px 88px 32px 32px;
    background:
      radial-gradient(80% 160% at 0% 0%, color-mix(in srgb, var(--sponsor) 26%, transparent) 0%, transparent 70%),
      linear-gradient(180deg, color-mix(in srgb, var(--sponsor) 10%, var(--l-modal-bg)) 0%, var(--l-modal-bg) 100%);
    box-shadow: inset 0 -1px 0 var(--l-modal-border);
  }

  &__mark {
    flex: none;
    display: grid;
    place-items: center;
    width: 64px;
    height: 64px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--sponsor) 16%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 40%, transparent);
    color: var(--sponsor);
    font-size: 26px;
  }

  &__intro {
    min-width: 0;
  }

  &__title {
    margin: 0;
    color: var(--l-text);
    font-size: 24px;
    font-weight: 700;
    line-height: 32px;
  }

  &__lead {
    margin: 8px 0 0;
    color: var(--l-text-dim);
    font-size: 14px;
    line-height: 20px;
    text-wrap: pretty;
  }

  &__body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 28px 32px 32px;
    overflow-y: auto;
  }

  &__section {
    margin: 0 0 12px;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  // Шесть дорожек: трон — все шесть, пьедестал — по три, остальные — по две.
  &__hill {
    display: grid;
    grid-template-columns: repeat(6, minmax(0, 1fr));
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  &__item {
    --avatar: 36px;
    --nick: 15px;
    --place: var(--l-text-faint);
    grid-column: span 2;
    min-width: 0;
  }

  &__item.is-podium {
    --avatar: 48px;
    --nick: 19px;
    grid-column: span 3;
  }

  &__item.is-podium:nth-child(2) { --place: #cfd8e3; }
  &__item.is-podium:nth-child(3) { --place: #e0a370; }

  // Один на пьедестале — на всю ширину, без пустой половины ряда.
  &__item.is-podium:nth-child(2):last-child { grid-column: 1 / -1; }

  &__item.is-king {
    --avatar: 76px;
    --nick: 28px;
    grid-column: 1 / -1;
    margin-bottom: 8px;
  }

  &__card {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    height: 100%;
    min-width: 0;
    margin: 0;
    padding: 12px 16px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: inherit;
    font: inherit;
    text-align: left;

    &.is-link {
      cursor: pointer;
      transition: background-color var(--l-fast), box-shadow var(--l-fast);

      &:hover {
        background: color-mix(in srgb, var(--sponsor) 8%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 40%, transparent);
      }

      &:focus-visible {
        outline: 2px solid var(--l-accent);
        outline-offset: 2px;
      }
    }
  }

  .is-podium &__card { padding: 14px 16px; gap: 14px; }

  // Трон: золотая подсветка и место под корону над аватаром.
  // Под троном ещё подпись, так что карточка не тянется на весь пункт.
  .is-king &__card {
    height: auto;
    gap: 20px;
    padding: 28px 24px 20px;
    background:
      radial-gradient(70% 140% at 0% 0%, color-mix(in srgb, var(--gold) 16%, transparent) 0%, transparent 70%),
      color-mix(in srgb, var(--gold) 4%, rgba(255, 255, 255, 0.03));
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--gold) 45%, transparent), 0 8px 32px color-mix(in srgb, var(--gold) 10%, transparent);

    &.is-link:hover {
      background:
        radial-gradient(70% 140% at 0% 0%, color-mix(in srgb, var(--gold) 22%, transparent) 0%, transparent 70%),
        color-mix(in srgb, var(--gold) 6%, rgba(255, 255, 255, 0.03));
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--gold) 70%, transparent), 0 8px 32px color-mix(in srgb, var(--gold) 16%, transparent);
    }
  }

  &__text {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  &__title-king {
    color: var(--gold);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  &__name {
    /* Свечение оформленного ника выходит за буквы, а overflow ради
       многоточия срезал бы его: поля под свечение съедаются отрицательным
       отступом, раскладка не меняется (как у ника в профиле). */
    margin: -8px -14px;
    padding: 8px 14px;
    overflow: hidden;
    color: var(--l-text);
    font-size: var(--nick);
    font-weight: 700;
    line-height: 1.3;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__amount {
    overflow: hidden;
    color: var(--l-text-dim);
    font-size: 12px;
    font-weight: 600;
    line-height: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .is-king &__amount {
    margin-top: 2px;
    color: var(--gold);
    font-size: 15px;
    line-height: 20px;
  }

  /* Сообщение — до двух строк (у трона до трёх), длинные слова и ссылки
     переносятся; целиком — в подсказке. */
  &__note {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    overflow: hidden;
    margin-top: 2px;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
    overflow-wrap: anywhere;
  }

  .is-king &__note {
    -webkit-line-clamp: 3;
    margin-top: 6px;
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 18px;
  }

  // Номер места в углу: серебро и бронза у пьедестала, серый у остальных.
  &__place {
    flex: none;
    align-self: flex-start;
    min-width: 22px;
    padding: 0 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--place) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--place) 40%, transparent);
    color: var(--place);
    font-size: 11px;
    font-weight: 700;
    line-height: 20px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  &__dethrone {
    margin: 8px 0 0;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
    text-align: center;
  }

  &__empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    min-height: 160px;
    padding: 24px;
    border-radius: var(--l-radius);
    background: rgba(255, 255, 255, 0.02);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 14px;
    line-height: 20px;
    text-align: center;

    i {
      color: color-mix(in srgb, var(--sponsor) 70%, var(--l-text-faint));
      font-size: 24px;
    }
  }
}

@media (prefers-reduced-motion: reduce) {
  .sponsors-modal__close,
  .sponsors-modal__cta,
  .sponsors-modal__person,
  .sponsors-modal__plate,
  .sponsors-modal__plain,
  .sponsors-modal__card {
    transition: none;
  }
}

/* Liquid Glass задаёт окнам свою тень (launcher.css): розовое свечение
   «Спонсорства» ложится поверх неё. */
.launcher-root[data-theme="liquid-glass"] .sponsors-modal {
  box-shadow: inset 0 0 16px rgba(255, 79, 216, 0.2), 0 0 28px rgba(255, 79, 216, 0.28), var(--lg-rim), var(--lg-shadow);
}
</style>
