<template>
  <div class="settings-modal" @mousedown.self="$emit('close')">
    <div
      class="settings-modal__window changelog__window"
      role="dialog"
      aria-modal="true"
      :aria-label="$t('#L_CHANGELOG')"
      ref="window"
    >
      <!-- Крестик — первым в окне: на него встаёт фокус при открытии. -->
      <button
        type="button"
        class="changelog__close"
        :title="$t('#L_CLOSE')"
        :aria-label="$t('#L_CLOSE')"
        @click="$emit('close')"
      >
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>

      <!-- Шапка как у профиля, без баннера: название и вкладки под ним. -->
      <header class="changelog__head" :class="{ 'has-tabs': hasGameNotes }">
        <h2 class="changelog__title">{{ $t("#L_CHANGELOG") }}</h2>

        <nav v-if="hasGameNotes" class="changelog__tabs" role="tablist" :aria-label="$t('#L_CHANGELOG')">
          <button
            v-for="tab in tabs"
            :key="tab.id"
            :ref="(el) => setTabRef(tab.id, el)"
            type="button"
            role="tab"
            class="changelog__tab"
            :class="{ 'is-active': activeTab === tab.id }"
            :aria-selected="activeTab === tab.id ? 'true' : 'false'"
            @click="activeTab = tab.id"
          >
            <i :class="tab.icon" aria-hidden="true"></i>
            <span>{{ $t(tab.title) }}</span>
          </button>
          <span v-if="ink" class="changelog__tab-ink l-ink" :class="'is-' + inkMotion" :style="inkStyle" aria-hidden="true"></span>
        </nav>
      </header>

      <!-- Вкладки — страницы в одной ячейке со своей прокруткой; при смене
           уезжают в сторону нажатой вкладки (l-slide-*), как в профиле. -->
      <div class="changelog__body" ref="body">

        <!-- Вкладка «Лаунчер»: история выпусков самого лаунчера -->
        <transition :name="'l-slide-' + slideDir">
          <div v-show="activeTab === 'launcher'" class="changelog__page">
            <section
              v-for="(release, index) in allReleases"
              :key="'launcher-' + index"
              :ref="'release-' + (release.version || index)"
              class="changelog__release"
              :class="{
                'changelog__release--focus': isFocused(release),
                'is-current': release.current,
                'is-upcoming': release.upcoming,
              }"
              :style="{ '--i': Math.min(index, 6) }"
            >
              <h3 class="changelog__release-title">
                <span class="changelog__release-name">{{ release.title }}</span>
                <span v-if="release.current" class="changelog__badge">{{
                  $t("#L_CL_CURRENT")
                }}</span>
                <span v-if="release.upcoming" class="changelog__badge is-upcoming">{{
                  $t("#L_CL_UPCOMING")
                }}</span>
              </h3>

              <div
                v-for="(group, gi) in release.groups"
                :key="gi"
                class="changelog__group"
              >
                <h4 v-if="group.title" class="changelog__group-title">
                  {{ group.title }}
                </h4>
                <ul class="changelog__items">
                  <li v-for="(item, ii) in group.items" :key="ii">{{ item }}</li>
                </ul>
              </div>
            </section>
          </div>
        </transition>

        <!-- Вкладка «Игры»: патчноуты сборок, по игре и версии -->
        <transition :name="'l-slide-' + slideDir">
          <div v-if="hasGameNotes" v-show="activeTab === 'games'" class="changelog__page">
            <section
              v-for="(game, index) in gameNotes"
              :key="'game-' + game.gameId"
              class="changelog__release"
              :style="{ '--i': Math.min(index, 6) }"
            >
              <h3 class="changelog__release-title">
                <span class="changelog__release-name">{{ gameTitle(game) }}</span>
              </h3>

              <div
                v-for="entry in game.versions"
                :key="entry.version"
                class="changelog__group"
              >
                <h4 class="changelog__group-title">
                  <span class="changelog__version">{{ entry.version }}</span>
                </h4>
                <ul class="changelog__items">
                  <li v-for="(item, ii) in entry.items" :key="ii">{{ item }}</li>
                </ul>
              </div>
            </section>
          </div>
        </transition>
      </div>
    </div>
  </div>
</template>

<script>
const { ipcRenderer } = window.hlsrElectron;
import changelog from "./changelog";
import uiState from "./ui-state";
import { trapFocus } from "./focus-trap";
import { inkBox, direction } from "./tab-ink";
import LocalizationHelper from "@/utils/locales";
import launcherData from "./launcher-data";

const VERSION_RE = /^[0-9A-Za-z.\-+]{1,32}$/;

export default {
  name: "ChangelogModal",
  emits: ["close"],
  props: {
    initialVersion: {
      type: String,
      default: null,
    },
    extraRelease: {
      type: Object,
      default: null,
    },
  },
  data() {
    return {
      activeTab: "launcher",
      gameNotes: [],
      ink: null,
      inkMotion: "still",
      slideDir: "next",
      tabs: [
        { id: "launcher", title: "#L_CL_TAB_LAUNCHER", icon: "fas fa-rocket" },
        { id: "games", title: "#L_CL_TAB_GAMES", icon: "fas fa-gamepad" },
      ],
      releases: changelog.buildChangelog(
        this.$t("#UI_CHANGELOG_CONTENT"),
        this.currentLanguage(),
        ipcRenderer.sendSync("app-version")
      ),
    };
  },
  computed: {
    inkStyle() {
      return { left: this.ink.left + "px", right: this.ink.right + "px" };
    },
    hasGameNotes() {
      return this.gameNotes.length > 0;
    },
    allReleases() {
      const extra = this.safeExtraRelease();
      if (!extra) return this.releases;

      if (this.releases.some((release) => release.version === extra.version))
        return this.releases;

      return [extra].concat(this.releases);
    },
  },
  watch: {
    activeTab(id, previous) {
      this.slideDir = direction(this.tabs.map((tab) => tab.id), previous, id);
      this.$nextTick(() => this.measureInk(this.slideDir));
    },
    // Вкладки появляются, когда пришли патчноуты игр.
    hasGameNotes() {
      this.$nextTick(() => this.measureInk("still"));
    },
  },
  created() {
    this.tabEls = {};
  },
  methods: {
    setTabRef(id, el) {
      if (el) this.tabEls[id] = el;
      else delete this.tabEls[id];
    },
    // Полоса под вкладкой (tab-ink.js), как в профиле. При изменении
    // размеров окна — без анимации.
    measureInk(motion) {
      const el = this.tabEls[this.activeTab];
      const box = inkBox(el && el.parentElement, el);
      if (!box) return;
      this.inkMotion = this.ink && typeof motion === "string" ? motion : "still";
      this.ink = box;
    },
    safeExtraRelease() {
      const raw = this.extraRelease;
      if (!raw || typeof raw !== "object") return null;
      if (typeof raw.version !== "string" || !VERSION_RE.test(raw.version)) return null;
      if (!Array.isArray(raw.items)) return null;

      const items = raw.items
        .filter((item) => typeof item === "string" && item.trim())
        .map((item) => item.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim().slice(0, 200))
        .filter(Boolean)
        .slice(0, 20);

      if (!items.length) return null;

      return {
        version: raw.version,
        title: `HLSR ${raw.version}`,
        upcoming: true,
        groups: [{ title: null, items }],
      };
    },
    gameTitle(game) {
      const name = game.launcherId ? launcherData.shortNames[game.launcherId] : null;
      return name || game.gameId;
    },
    async loadGameNotes() {
      let list = null;

      try {
        list = await ipcRenderer.invoke("game-release-notes-all");
      } catch (error) {
        list = null;
      }

      if (!Array.isArray(list)) return;

      this.gameNotes = list
        .filter((game) => game && typeof game.gameId === "string" && Array.isArray(game.versions))
        .map((game) => ({
          gameId: game.gameId,
          launcherId: typeof game.launcherId === "string" ? game.launcherId : null,
          versions: game.versions
            .filter((entry) => entry && VERSION_RE.test(String(entry.version)) && Array.isArray(entry.items))
            .map((entry) => ({
              version: String(entry.version),
              items: entry.items
                .filter((item) => typeof item === "string" && item.trim())
                .map((item) => item.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim())
                .filter(Boolean),
            }))
            .filter((entry) => entry.items.length),
        }))
        .filter((game) => game.versions.length);
    },
    isFocused(release) {
      return !!this.initialVersion && release.version === this.initialVersion;
    },
    currentLanguage() {
      try {
        return Object.keys(LocalizationHelper.locales)[LocalizationHelper.lang] === "en"
          ? "en"
          : "ru";
      } catch (e) {
        return "ru";
      }
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
    scrollToInitial() {
      if (!this.initialVersion) return;

      const refs = this.$refs["release-" + this.initialVersion];
      const el = Array.isArray(refs) ? refs[0] : refs;
      if (el && typeof el.scrollIntoView === "function")
        el.scrollIntoView({ block: "start" });
    },
  },
  mounted() {
    document.addEventListener("keydown", this.onKeydown);
    window.addEventListener("resize", this.measureInk);
    this.loadGameNotes();
    this.$nextTick(() => {
      const btn = this.$refs.window.querySelector("button");
      if (btn) btn.focus();
      this.scrollToInitial();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    window.removeEventListener("resize", this.measureInk);
  },
};
</script>

<style lang="scss" scoped>
.settings-modal {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;

  &__window {
    position: relative;
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border: none;
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }
}

.changelog {
  &__window {
    width: min(760px, calc(100% - 80px));
    height: min(680px, calc(100% - 80px));
  }

  /* Крестик — как в профиле. */
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

  &__head {
    position: relative;
    flex: none;
    padding: 24px 32px 24px;
    box-shadow: inset 0 -1px 0 var(--l-modal-border);

    &.has-tabs { padding-bottom: 0; }
  }

  &__title {
    margin: 0;
    padding-right: 56px;
    color: var(--l-text);
    font-size: 24px;
    font-weight: 700;
    line-height: 40px;
    letter-spacing: 0.01em;
    animation: l-rise-in var(--l-dur-slow) var(--l-ease) both;
  }

  /* ---- Вкладки (как в профиле) ---- */

  &__tabs {
    position: relative;
    display: flex;
    gap: 8px;
    margin-top: 12px;
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
      transition: color var(--l-fast), transform var(--l-dur) var(--l-ease-spring);
    }

    &:hover {
      color: var(--l-text);

      i { transform: scale(1.12); }
    }

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

  &__tab-ink {
    bottom: 0;
    height: 2px;
    border-radius: 2px 2px 0 0;
    background: var(--l-accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 60%, transparent);
  }

  /* ---- Содержимое ---- */

  &__body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    overflow: hidden;
  }

  &__page {
    grid-area: 1 / 1;
    min-height: 0;
    box-sizing: border-box;
    padding: 24px 32px 32px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* Выпуск — карточкой, как блок забега в профиле. Текущий выпуск
     подсвечен мягким свечением акцента в углу. Карточки поднимаются
     по очереди при открытии окна. */
  &__release {
    --release-tint: var(--l-text-faint);
    flex: none;
    padding: 20px 24px;
    border-radius: var(--l-radius);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
    transition: box-shadow var(--l-fast), background-color var(--l-fast);
    animation: l-rise-in var(--l-dur-slow) var(--l-ease) both;
    animation-delay: calc(var(--i, 0) * 40ms);

    &:hover {
      background-color: rgba(255, 255, 255, 0.04);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 30%, var(--l-border));
    }

    &.is-current,
    &.is-upcoming {
      background:
        radial-gradient(120% 140% at 100% 0%, color-mix(in srgb, var(--l-accent) 12%, transparent) 0%, transparent 60%),
        rgba(255, 255, 255, 0.03);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 26%, var(--l-border));
    }
  }

  /* Выпуск, к которому открыли окно (из карточки обновления). */
  &__release--focus {
    box-shadow: inset 0 0 0 1px var(--l-accent), 0 0 0 3px var(--accent-soft);
  }

  &__release-title {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin: 0 0 12px;
    color: var(--l-text);
    font-size: 16px;
    font-weight: 700;
    line-height: 24px;
  }

  &__release-name { min-width: 0; }

  &__badge {
    padding: 0 8px;
    border-radius: 6px;
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1px var(--accent-border);
    color: var(--accent);
    font-size: 10.5px;
    font-weight: 700;
    line-height: 20px;
    letter-spacing: 0.08em;
    text-transform: uppercase;

    &.is-upcoming {
      background: rgba(255, 179, 122, 0.12);
      box-shadow: inset 0 0 0 1px rgba(255, 179, 122, 0.35);
      color: #ffb37a;
    }
  }

  &__group + &__group { margin-top: 16px; }

  &__group-title {
    margin: 0 0 8px;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  /* Версия сборки — плашкой, как ID в профиле. */
  &__version {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    letter-spacing: 0.04em;
    font-variant-numeric: tabular-nums;
  }

  /* Свои маркеры: точка акцента вместо системного кружка. */
  &__items {
    margin: 0;
    padding: 0;
    list-style: none;
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 22px;

    li {
      position: relative;
      padding-left: 18px;
      word-break: break-word;

      &::before {
        content: "";
        position: absolute;
        left: 2px;
        top: 8px;
        width: 6px;
        height: 6px;
        border-radius: 50%;
        background: color-mix(in srgb, var(--l-accent) 70%, transparent);
      }
    }

    li + li { margin-top: 6px; }
  }
}

@media (prefers-reduced-motion: reduce) {
  .changelog__tab-ink,
  .changelog__title,
  .changelog__release {
    transition: none;
    animation: none;
  }
}
</style>
