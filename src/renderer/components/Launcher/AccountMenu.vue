<template>
  <div class="account-menu" ref="root">
    <button
      type="button"
      ref="toggle"
      class="account-menu__toggle"
      :class="{ open: open }"
      :title="accountLabel + ' · ' + presenceText + (account.signedIn ? ' · ' + statusText : '')"
      :aria-label="accountLabel"
      aria-haspopup="menu"
      :aria-expanded="open ? 'true' : 'false'"
      @click="toggle"
      @keydown.down.prevent="openAndFocus(0)"
    >
      <AvatarFrame :frame="showNick ? frame : null" :width="2" hole="var(--l-bg)">
        <span class="account-avatar">
          <img v-if="account.avatarDataUri" :src="account.avatarDataUri" alt="" />
          <i v-else class="fab fa-steam" aria-hidden="true"></i>
          <span class="account-avatar__dot" :class="'is-' + presenceState"></span>
        </span>
      </AvatarFrame>
      <span class="account-menu__identity">
        <span class="account-menu__name"><NickName :nick="showNick ? nick : null" :subscriber="showNick && subscriber">{{ accountName }}</NickName></span>
        <span class="account-menu__connection">{{ presenceText }}</span>
      </span>
      <i class="fas fa-chevron-down account-menu__chev" aria-hidden="true"></i>
    </button>

    <transition name="l-pop">
      <div v-if="open" class="account-menu__panel" role="menu">
        <div class="account-menu__items">
          <button
            v-for="(item, i) in items"
            :key="item.id"
            ref="options"
            type="button"
            role="menuitem"
            class="account-menu__item"
            :style="{ '--i': i + 1 }"
            :disabled="item.disabled"
            @click="select(item)"
            @keydown="onItemKeydown($event, i)"
          >
            <span class="account-menu__item-icon" aria-hidden="true"><i :class="item.icon"></i></span>
            <span>{{ item.label }}</span>
          </button>
        </div>
      </div>
    </transition>
  </div>
</template>

<script>
import orderedRefs from "@/utils/orderedRefs";
import uiState from "./ui-state";
import NickName from "./NickName";
import AvatarFrame from "./AvatarFrame";
import launcherData from "./launcher-data";
const { ipcRenderer } = window.hlsrElectron;

const ACCOUNT_REFRESH_MS = 30 * 1000;

export default {
  emits: ["action"],
  name: "AccountMenu",
  components: { NickName, AvatarFrame },
  data() {
    return {
      open: false,
      account: {
        signedIn: false,
        steamId64: null,
        steamId2: null,
        personaName: null,
        avatarDataUri: null,
        connection: "checking",
      },
      nick: null,
      // Подписка активна сейчас: значок у ника.
      subscriber: false,
      frame: null,
      presence: { online: false, gameId: null, launcherId: null },
      accountKnown: false,
      accountTimer: null,
    };
  },
  computed: {
    connectionState() {
      const value = this.account && this.account.connection;
      return ["connected", "elevated", "offline", "checking"].indexOf(value) === -1
        ? "unknown"
        : value;
    },
    accountLoading() {
      return !this.accountKnown || this.account.connection === "checking";
    },
    accountName() {
      if (this.accountLoading) return this.$t("#L_SB_STEAM_LOADING");
      if (!this.account.signedIn) return this.$t("#L_RUNNER_LOGIN");
      return this.account.personaName || this.$t("#L_SB_STEAM_ACCOUNT");
    },
    // Оформление ника — только у вошедшего: у «Войти через Steam» и
    // «Загрузка…» его нет.
    showNick() {
      return !this.accountLoading && this.account.signedIn;
    },
    accountLabel() {
      return !this.accountLoading && !this.account.signedIn
        ? this.$t("#L_RUNNER_LOGIN")
        : this.accountName;
    },
    statusText() {
      if (this.accountKnown && !this.account.signedIn) return this.$t("#L_RUNNER_GUEST");
      if (this.accountLoading) return this.$t("#L_SB_STEAM_LOADING");

      switch (this.connectionState) {
        case "connected":
          return this.$t("#L_RUNNER_ONLINE");
        case "checking":
          return this.$t("#L_ST_CHECKING");
        case "elevated":
          return this.$t("#L_ST_ELEVATED");
        case "offline":
          return this.$t("#L_RUNNER_OFFLINE");
        default:
          return this.$t("#L_ST_UNKNOWN");
      }
    },
    // Точка у аватара: зелёная в сети, серая без связи с HLSR
    presenceState() {
      if (this.accountKnown && !this.account.signedIn) return "unknown";
      if (this.accountLoading) return "checking";
      return this.presence.online ? "online" : "offline";
    },
    presenceText() {
      if (this.accountKnown && !this.account.signedIn) return this.$t("#L_RUNNER_GUEST");
      if (this.accountLoading) return this.$t("#L_ST_CHECKING");
      const game = this.presence.launcherId && launcherData.shortNames[this.presence.launcherId];
      if (game) return this.$t("#L_PRESENCE_PLAYING", game);
      return this.$t(this.presence.online ? "#L_PRESENCE_ONLINE" : "#L_PRESENCE_OFFLINE");
    },
    canSignIn() {
      return this.accountKnown && !this.accountLoading && !this.account.signedIn;
    },
    statusShort() {
      if (this.accountKnown && !this.account.signedIn) return this.$t("#L_RUNNER_GUEST");
      switch (this.connectionState) {
        case "connected":
          return "";
        case "checking":
          return this.$t("#L_ST_CHECKING");
        case "elevated":
          return this.$t("#L_ACC_STEAM_ELEVATED");
        case "offline":
          return this.$t("#L_RUNNER_OFFLINE");
        default:
          return this.$t("#L_ACC_STEAM_UNKNOWN");
      }
    },
    items() {
      // Гостю первый пункт — вход (открывает тот же экран входа, что и профиль).
      const list = [this.canSignIn
        ? { id: "open-profile", icon: "fab fa-steam", label: this.$t("#L_RUNNER_LOGIN") }
        : { id: "open-profile", icon: "fas fa-user", label: this.$t("#L_SB_MENU_OPEN_PROFILE") }];
      if (this.account.signedIn) {
        list.push({ id: "profile-settings", icon: "fas fa-sliders-h", label: this.$t("#L_SB_MENU_PROFILE_SETTINGS") });
      }
      list.push({ id: "friends", icon: "fas fa-user-friends", label: this.$t("#L_FRIENDS_TITLE") });
      return list;
    },
  },
  methods: {
    toggle() {
      this.open ? this.close() : this.show();
    },
    show() {
      if (this.open) return;
      this.open = true;
      uiState.openDropdowns++;
      document.addEventListener("mousedown", this.onDocumentDown, true);
      document.addEventListener("keydown", this.onDocumentKey, true);
    },
    close(focusToggle) {
      if (!this.open) return;
      this.open = false;
      uiState.openDropdowns = Math.max(0, uiState.openDropdowns - 1);
      document.removeEventListener("mousedown", this.onDocumentDown, true);
      document.removeEventListener("keydown", this.onDocumentKey, true);
      if (focusToggle) this.$refs.toggle.focus();
    },
    openAndFocus(index) {
      this.show();
      this.$nextTick(() => this.focusOption(index));
    },
    menuOptions() {
      return orderedRefs(this.$refs.options).filter((el) => !el.disabled);
    },
    focusOption(index) {
      const options = this.menuOptions();
      if (options.length) options[(index + options.length) % options.length].focus();
    },
    select(item) {
      this.close(true);
      this.$emit("action", item.id);
    },
    onDocumentDown(e) {
      if (!this.$refs.root.contains(e.target)) this.close();
    },
    onDocumentKey(e) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      this.close(true);
    },
    onItemKeydown(e) {
      const options = this.menuOptions();
      const index = options.indexOf(e.target);

      if (e.key === "ArrowDown") {
        e.preventDefault();
        this.focusOption(index + 1);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        this.focusOption(index - 1);
      } else if (e.key === "Tab") {
        this.close();
      }
    },
    async refreshAccount() {
      try {
        const value = await ipcRenderer.invoke("runner-state");

        if (value && typeof value === "object") {
          this.account = value.account || { signedIn: false };
          this.nick = value.nick || null;
          this.subscriber = value.subscriber === true;
          this.frame = value.avatarFrame || null;
          this.presence = value.presence || this.presence;
          this.accountKnown = true;
        }
      } catch (ignored) {
      }
    },
    onAccountPushed(event, value) {
      if (!value || typeof value !== "object") return;

      this.account = value.account || { signedIn: false };
      this.nick = value.nick || null;
      this.subscriber = value.subscriber === true;
      this.frame = value.avatarFrame || null;
      this.presence = value.presence || this.presence;
      this.accountKnown = true;
    },
  },
  mounted() {
    ipcRenderer.on("runner-profile-state", this.onAccountPushed);
    this.refreshAccount();
    this.accountTimer = setInterval(this.refreshAccount, ACCOUNT_REFRESH_MS);
    window.addEventListener("focus", this.refreshAccount);
  },
  beforeUnmount() {
    this.close();
    if (this.accountTimer) clearInterval(this.accountTimer);
    this.accountTimer = null;

    ipcRenderer.removeListener("runner-profile-state", this.onAccountPushed);
    window.removeEventListener("focus", this.refreshAccount);
  },
};
</script>

<style lang="scss" scoped>
@use "./round-dot" as *;

.account-menu {
  position: relative;
  -webkit-app-region: no-drag;

  &__toggle {
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--l-header-control);
    width: 248px;
    padding: 0 8px 0 0;
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-text);
    font-family: inherit;
    cursor: pointer;
    transition: background var(--l-fast);

    &:hover,
    &.open {
      background: rgba(255, 255, 255, 0.06);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 0;
    }
  }

  /* Имя и состояние Steam привязаны к аватару общей колонкой. */
  &__identity {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    text-align: left;
  }

  &__name {
    /* Свечение оформленного ника выходит за буквы, а блок обрезает всё
       по краю ради многоточия: поля под свечение тут же съедаются
       отрицательным отступом, раскладка не меняется. */
    margin: -6px -8px;
    padding: 6px 8px;
    font-size: 15px;
    font-weight: 500;
    line-height: 24px;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__connection {
    color: var(--l-text-dim);
    font-size: 11px;
    line-height: 16px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Ширина стрелки — целое число шагов: глиф шириной 9.63 px сдвигал бы
     правый край кнопки с сетки */
  &__chev {
    flex: none;
    width: 12px;
    font-size: 11px;
    line-height: 16px;
    text-align: center;
    color: var(--l-text-dim);
    transition: transform var(--l-fast);
  }

  &__toggle.open &__chev {
    transform: rotate(180deg);
  }

  /* Меню выровнено по правому краю кнопки и не уже 240 px: по ширине
     одного ника оно получалось кривым обрезком. Поверхность и рамка — как
     у окна профиля; рамка тенью, а не border: border сдвинул бы всё
     содержимое на пиксель с сетки. */
  &__panel {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    width: 260px;
    padding: 8px;
    border-radius: var(--l-radius);
    background: var(--l-modal-bg);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
    transform-origin: 100% 0;
    z-index: 40;
  }

  &__items {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* Пункты — как в меню игры: значок в плашке, наведение подсвечивает
     рамку и фон. */
  &__item {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 8px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: none;
    box-shadow: inset 0 0 0 1px transparent;
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 14px;
    font-weight: 500;
    line-height: 20px;
    text-align: left;
    cursor: pointer;
    transition: background-color var(--l-fast), box-shadow var(--l-fast), color var(--l-fast);
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
    animation-delay: calc(var(--i, 0) * 24ms);

    &:hover:not(:disabled),
    &:focus-visible {
      background: rgba(255, 255, 255, 0.05);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border));
      color: var(--l-text);
      outline: none;
    }

    &:active:not(:disabled) {
      background: rgba(255, 255, 255, 0.08);
    }

    &:disabled {
      opacity: 0.45;
      cursor: default;
    }
  }

  &__item-icon {
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
    transition: background-color var(--l-fast), color var(--l-fast), box-shadow var(--l-fast), transform var(--l-dur) var(--l-ease-spring);

    > i { line-height: 1; }
  }

  &__item:hover:not(:disabled) &__item-icon,
  &__item:focus-visible &__item-icon {
    background: var(--l-accent-soft);
    box-shadow: inset 0 0 0 1px transparent;
    color: var(--l-accent);
    transform: scale(1.06);
  }
}

/* Кольцо — outline внутрь, а не border: outline рисуется поверх картинки
   и не сдвигает ни её, ни точку статуса с сетки на пиксель */
.account-avatar {
  position: relative;
  flex: 0 0 var(--l-header-control);
  width: var(--l-header-control);
  height: var(--l-header-control);
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.05);
  outline: 1px solid var(--l-border);
  outline-offset: -1px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--l-text-faint);
  font-size: 24px;

  img {
    width: 100%;
    height: 100%;
    border-radius: 50%;
    object-fit: cover;
    display: block;
  }

  /* Цвет точки — онлайн в HLSR, та же палитра, что в меню */
  &__dot {
    position: absolute;
    /* Обводка 2px теперь внутри блока: блок 16px вместо 12px и сдвинут на 2px */
    right: -2px;
    bottom: -2px;
    width: 16px;
    height: 16px;
    @include round-dot(#8b939e, var(--l-bg), 2px);

    &.is-checking { --dot-color: #e8c33d; }
    &.is-online { --dot-color: #2fa84f; }
    &.is-offline { --dot-color: #8b939e; }
  }
}

/* Шапка обязана влезать в самое узкое окно (1152 px, window-size.js).
   До 1248 px — только аватар и стрелка: ник и состояние есть в подсказке
   и в открытом меню. Ширины кратны 4. */
@media (max-width: 1247px) {
  .account-menu__toggle {
    width: calc(var(--l-header-control) + 32px);
  }

  .account-menu__identity {
    display: none;
  }
}
</style>
