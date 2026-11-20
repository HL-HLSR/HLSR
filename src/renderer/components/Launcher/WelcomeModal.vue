<template>
  <div class="settings-modal welcome">
    <div
      ref="window"
      class="settings-modal__window welcome__window"
      role="dialog"
      aria-modal="true"
      aria-labelledby="welcome-title"
    >
      <div class="welcome__body">
        <p class="welcome__kicker">HLSR {{ version }}</p>
        <h2 id="welcome-title" class="welcome__title">{{ $t("#L_WELCOME_TITLE") }}</h2>
        <p class="welcome__lead">{{ $t("#L_WELCOME_LEAD") }}</p>

        <ul class="welcome__perks">
          <li v-for="perk in perks" :key="perk.key">
            <i :class="perk.icon" aria-hidden="true"></i>
            <span>{{ $t(perk.key) }}</span>
          </li>
        </ul>

        <label v-if="official" class="welcome__consent">
          <input ref="consent" v-model="agreed" type="checkbox" />
          <span class="welcome__box" aria-hidden="true"><i class="fas fa-check"></i></span>
          <span>
            {{ $t("#L_WELCOME_CONSENT_A") }}
            <button type="button" class="welcome__link" @click.prevent="openLink('privacy')">{{ $t("#L_WELCOME_PRIVACY") }}</button>
            {{ $t("#L_WELCOME_CONSENT_B") }}
            <button type="button" class="welcome__link" @click.prevent="openLink('terms')">{{ $t("#L_WELCOME_TERMS") }}</button>
          </span>
        </label>

        <div class="welcome__actions">
          <OfficialStub v-if="!official" text="#L_OFFICIAL_LOGIN" />
          <button
            v-else
            type="button"
            class="welcome__button is-primary"
            :disabled="!agreed || busy"
            @click="signIn"
          >
            <i :class="busy ? 'fas fa-spinner fa-spin' : 'fab fa-steam'" aria-hidden="true"></i>
            {{ $t("#L_RUNNER_LOGIN") }}
          </button>
          <button type="button" class="welcome__button" :disabled="busy" @click="later">
            {{ $t("#L_WELCOME_LATER") }}
          </button>
        </div>
        <p v-if="error" class="welcome__error" role="alert">{{ $t("#L_RUNNER_NETWORK") }}</p>
        <p class="welcome__local">{{ $t("#L_WELCOME_LOCAL") }}</p>
      </div>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";
import OfficialStub from "./OfficialStub";
import { OFFICIAL } from "../../official";

const { ipcRenderer } = window.hlsrElectron;

// Первый запуск 0.4 и новые установки: предложение войти через Steam.
// «Войти позже» оставляет локальный профиль: игры запускаются как обычно,
// но без облачной синхронизации, часов в профиле и онлайн-функций. Войти
// можно в любой момент через аватар в углу.
export default {
  name: "WelcomeModal",
  components: { OfficialStub },
  emits: ["signed-in", "later"],
  props: {
    version: { type: String, default: "" },
  },
  data() {
    return {
      official: OFFICIAL,
      agreed: false,
      busy: false,
      error: false,
      lastFocused: null,
      perks: [
        { key: "#L_WELCOME_PERK_SYNC", icon: "fas fa-cloud" },
        { key: "#L_WELCOME_PERK_PROFILE", icon: "fas fa-id-badge" },
        { key: "#L_WELCOME_PERK_FRIENDS", icon: "fas fa-user-friends" },
        { key: "#L_WELCOME_PERK_FEED", icon: "fas fa-newspaper" },
      ],
    };
  },
  methods: {
    openLink(key) {
      ipcRenderer.invoke("open-external-link", key);
    },
    async signIn() {
      if (!this.agreed || this.busy) return;
      this.busy = true;
      this.error = false;
      try {
        const result = await ipcRenderer.invoke("runner-login", true);
        if (!result || result.status === "error") {
          this.error = true;
          return;
        }
        this.$emit("signed-in");
      } catch (error) {
        this.error = true;
      } finally {
        this.busy = false;
      }
    },
    later() {
      this.$emit("later");
    },
    onKeydown(e) {
      // Окно нельзя закрыть мимо выбора: Esc значит «Войти позже».
      if (e.key === "Escape") {
        e.preventDefault();
        this.later();
      } else if (e.key === "Tab") {
        trapFocus(this.$refs.window, e);
      }
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);
    this.$nextTick(() => this.$refs.consent && this.$refs.consent.focus({ preventScroll: true }));
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
// Каркас модалки: стили scoped, поэтому каждое окно лаунчера держит свою копию.
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
  }
}

.welcome__window {
  width: min(760px, calc(100% - 64px));
  max-height: calc(100% - 64px);
  overflow-y: auto;
  padding: 0;
}

.welcome__body { padding: 32px 40px 32px; }

.welcome__kicker {
  margin: 0;
  color: var(--l-text-faint);
  font-size: 11.5px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}

.welcome__title {
  margin: 4px 0 8px;
  color: var(--l-text);
  font-size: 30px;
  line-height: 36px;
  font-weight: 800;
  letter-spacing: -0.01em;
}

.welcome__lead { margin: 0 0 16px; color: var(--l-text-dim); font-size: 13.5px; line-height: 22px; }

.welcome__perks {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  margin: 0 0 20px;
  padding: 0;
  list-style: none;
  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text);
    font-size: 12.5px;
    line-height: 18px;
  }
  i { width: 16px; color: var(--accent); text-align: center; }
}

.welcome__consent {
  position: relative;
  display: flex;
  align-items: flex-start;
  gap: 10px;
  color: var(--l-text-dim);
  font-size: 12.5px;
  line-height: 20px;
  cursor: pointer;
  input { position: absolute; opacity: 0; width: 18px; height: 18px; margin: 1px 0 0; }
}

.welcome__box {
  flex: none;
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  margin-top: 1px;
  border-radius: 5px;
  box-shadow: inset 0 0 0 1.5px var(--l-border-strong);
  color: transparent;
  font-size: 10px;
  transition: background var(--l-fast), box-shadow var(--l-fast), color var(--l-fast);
}
.welcome__consent input:checked + .welcome__box {
  background: var(--accent);
  box-shadow: none;
  color: var(--accent-contrast-text);
}
.welcome__consent input:focus-visible + .welcome__box { outline: 2px solid var(--accent-focus, var(--accent)); outline-offset: 2px; }

.welcome__link {
  padding: 0;
  border: 0;
  background: none;
  color: var(--l-accent, var(--accent));
  font: inherit;
  text-decoration: underline;
  text-underline-offset: 3px;
  cursor: pointer;
  &:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
}

.welcome__actions { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 20px; }

.welcome__button {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-height: 44px;
  padding: 10px 20px;
  border: 0;
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  box-shadow: inset 0 0 0 1px var(--l-border-strong);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13.5px;
  font-weight: 600;
  cursor: pointer;
  transition: background var(--l-fast), opacity var(--l-fast);
  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.09); }
  &:disabled { opacity: 0.45; cursor: default; }
  &:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  &.is-primary { background: var(--accent); box-shadow: none; color: var(--accent-contrast-text); }
  &.is-primary:hover:not(:disabled) { background: var(--accent-hover); }
}

.welcome__error { margin: 12px 0 0; color: #ff6b6b; font-size: 12.5px; }
.welcome__local { margin: 16px 0 0; color: var(--l-text-faint); font-size: 12px; line-height: 18px; }

@media (max-width: 560px) { .welcome__perks { grid-template-columns: 1fr; } }
</style>
