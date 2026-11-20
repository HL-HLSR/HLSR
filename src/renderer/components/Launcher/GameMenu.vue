<template>
  <div class="game-menu" ref="root">
    <button
      ref="trigger"
      type="button"
      class="game-menu__trigger"
      :class="{ open: open }"
      :title="$t('#L_GM_TITLE')"
      :aria-label="$t('#L_GM_TITLE')"
      aria-haspopup="menu"
      :aria-expanded="open ? 'true' : 'false'"
      @click="toggle"
      @keydown.up.prevent="openMenu(-1)"
      @keydown.down.prevent="openMenu(0)"
    >
      <i class="fas fa-bars" aria-hidden="true"></i>
    </button>

    <transition name="gm">
      <div
        v-if="open"
        ref="dropdown"
        class="game-menu__dropdown"
        :class="fit.size"
        :style="{ maxHeight: fit.maxHeight ? fit.maxHeight + 'px' : null }"
        role="menu"
        :aria-label="$t('#L_GM_TITLE')"
        @keydown.esc.prevent="close(true)"
        @keydown.down.prevent="move(1)"
        @keydown.up.prevent="move(-1)"
        @keydown.tab="onTab"
      >
        <button
          v-for="(item, index) in items"
          :key="item.id"
          ref="option"
          type="button"
          class="game-menu__item"
          :class="{ danger: item.danger, separated: item.separated }"
          :style="{ '--i': index }"
          role="menuitem"
          :disabled="!item.enabled"
          :title="item.enabled ? item.label : item.hint"
          :aria-disabled="item.enabled ? 'false' : 'true'"
          @click="choose(item)"
          @focus="active = index"
        >
          <span class="game-menu__icon" aria-hidden="true"><i class="fas" :class="item.icon"></i></span>
          <span class="game-menu__label">{{ item.label }}</span>
        </button>
      </div>
    </transition>
  </div>
</template>

<script>
import orderedRefs from "@/utils/orderedRefs";
import uiState from "./ui-state";

const { ipcRenderer } = window.hlsrElectron;

const ITEMS = [
  { id: "settings", icon: "fa-sliders-h", label: "#L_GM_SETTINGS" },
  { id: "shortcut", icon: "fa-external-link-alt", label: "#L_GM_SHORTCUT" },
  { id: "screenshots", icon: "fa-image", label: "#L_GM_SCREENSHOTS" },
  { id: "folder", icon: "fa-folder-open", label: "#L_GM_FOLDER" },
  { id: "verify", icon: "fa-check-double", label: "#L_GM_VERIFY" },
  { id: "repair", icon: "fa-wrench", label: "#L_GM_REPAIR" },
  { id: "exportBuild", icon: "fa-file-archive", label: "#L_GM_EXPORT_BUILD", separated: true },
  { id: "exportSettings", icon: "fa-file-export", label: "#L_GM_EXPORT_SETTINGS" },
  { id: "importBuild", icon: "fa-file-import", label: "#L_GM_IMPORT_BUILD" },
  { id: "uninstall", icon: "fa-trash-alt", label: "#L_GM_UNINSTALL", danger: true, separated: true },
];

const REASONS = {
  "not-installed": "#L_GM_DISABLED_NOT_INSTALLED",
  busy: "#L_GM_DISABLED_BUSY",
  "game-running": "#L_GM_DISABLED_GAME_RUNNING",
  "steam-managed": "#L_GM_DISABLED_STEAM_MANAGED",
  "in-development": "#L_GM_DISABLED_IN_DEVELOPMENT",
  "episode-not-installed": "#L_GM_DISABLED_EPISODE",
};

export default {
  name: "GameMenu",
  emits: ["action", "close", "open"],
  props: {
    gameId: {
      type: String,
      default: null,
    },
    // Выбран эпизод Ghosting: { installed }. «Удалить» тогда снимает только эпизод.
    episode: {
      type: Object,
      default: null,
    },
  },
  data() {
    return {
      open: false,
      active: 0,
      state: null,
      fit: { size: "", maxHeight: 0 },
    };
  },
  computed: {
    items() {
      let state = this.state && this.state.items ? this.state.items : {};

      return ITEMS.map((item) => {
        let info = state[item.id] || { enabled: false, reason: "not-installed" };
        // Удаление эпизода: те же запреты, что у игры (запущена, занято), и сам эпизод скачан.
        const episode = item.id === "uninstall" ? this.episode : null;
        if (episode && info.enabled && !episode.installed) info = { enabled: false, reason: "episode-not-installed" };

        return {
          id: item.id,
          icon: item.icon,
          label: this.$t(episode ? "#L_GM_UNINSTALL_EPISODE" : item.label),
          enabled: !!info.enabled,
          danger: !!item.danger,
          separated: !!item.separated,
          hint: info.reason ? this.$t(REASONS[info.reason] || "#L_GM_DISABLED_BUSY") : "",
        };
      });
    },
  },
  methods: {
    async refreshState() {
      if (!this.gameId) {
        this.state = null;
        return;
      }

      this.state = await ipcRenderer.invoke("game-menu-state", this.gameId);
    },
    toggle() {
      if (this.open) this.close(true);
      else this.openMenu(0);
    },
    async openMenu(index) {
      await this.refreshState();

      this.open = true;
      this.active = index < 0 ? this.items.length - 1 : index;

      uiState.openDropdowns++;
      this.$emit("open");

      this.$nextTick(() => {
        document.addEventListener("mousedown", this.onDocumentDown, true);
        window.addEventListener("resize", this.fitToWindow);
        this.fitToWindow();
        this.focusActive();
      });
    },
    close(returnFocus) {
      if (!this.open) return;

      this.open = false;
      uiState.openDropdowns = Math.max(0, uiState.openDropdowns - 1);
      document.removeEventListener("mousedown", this.onDocumentDown, true);
      window.removeEventListener("resize", this.fitToWindow);
      this.$emit("close");

      if (returnFocus && this.$refs.trigger) this.$refs.trigger.focus();
    },
    // Меню открывается вверх от кнопки и не должно заходить под верхнюю
    // панель: в низком окне пункты сжимаются по ступеням (compact, затем
    // tight), а если места нет и так — список прокручивается.
    fitToWindow() {
      let trigger = this.$refs.trigger;
      let dropdown = this.$refs.dropdown;
      if (!trigger || !dropdown) return;

      let topbar = document.querySelector(".launcher-topbar");
      let top = topbar ? topbar.getBoundingClientRect().bottom : 0;
      let box = trigger.getBoundingClientRect();
      // Меню — прямо над кнопкой, к её правому краю (просьба владельца 14.11: не над «Версией сборки»).
      let room = Math.floor(box.top - 12 - top - 8);

      let sizes = ["", "compact", "tight"];
      let attempt = (index) => {
        this.fit = { size: sizes[index], maxHeight: 0 };
        this.$nextTick(() => {
          if (!this.$refs.dropdown || this.$refs.dropdown.scrollHeight <= room) return;
          if (index + 1 < sizes.length) attempt(index + 1);
          else this.fit = { size: sizes[index], maxHeight: Math.max(160, room) };
        });
      };
      attempt(0);
    },
    onDocumentDown(event) {
      if (this.$refs.root && this.$refs.root.contains(event.target)) return;
      this.close(false);
    },
    focusActive() {
      let options = orderedRefs(this.$refs.option);
      let target = options[this.active];
      if (target) target.focus();
    },
    move(delta) {
      let total = this.items.length;
      let next = this.active;

      for (let step = 0; step < total; step++) {
        next = (next + delta + total) % total;
        if (this.items[next].enabled) break;
      }

      this.active = next;
      this.focusActive();
    },
    onTab(event) {
      this.close(false);
      event.stopPropagation();
    },
    choose(item) {
      if (!item.enabled) return;

      this.close(true);
      this.$emit("action", item.id);
    },
  },
  watch: {
    gameId() {
      this.close(false);
      this.refreshState();
    },
  },
  mounted() {
    this.refreshState();
  },
  beforeUnmount() {
    if (this.open) uiState.openDropdowns = Math.max(0, uiState.openDropdowns - 1);
    document.removeEventListener("mousedown", this.onDocumentDown, true);
    window.removeEventListener("resize", this.fitToWindow);
  },
};
</script>

<style lang="scss" scoped>
.game-menu {
  position: relative;
  display: flex;
  align-items: center;

  /* Высота — как у «Играть» рядом (ступень сетки в launcher.css) */
  &__trigger {
    width: 64px;
    min-width: 64px;
    height: var(--l-play-h);
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--l-radius);
    border: none;
    background: var(--l-surface-solid);
    /* Рамка — как у панели выбора сборки: тенью, при наведении и открытом
       меню берёт оттенок акцента. */
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text);
    font-size: 20px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);

    > i {
      width: 24px;
      line-height: 24px;
      text-align: center;
    }

    &:hover {
      background: linear-gradient(rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.08)),
        var(--l-surface-solid);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 35%, var(--l-border));
      color: #fff;
    }

    &:focus-visible {
      outline: none;
      background: linear-gradient(rgba(var(--accent-rgb), 0.22), rgba(var(--accent-rgb), 0.22)),
        var(--l-surface-solid);
      box-shadow: inset 0 0 0 1px var(--l-accent);
    }

    &:active,
    &.open {
      background: linear-gradient(rgba(var(--accent-rgb), 0.16), rgba(var(--accent-rgb), 0.16)),
        var(--l-surface-solid);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 35%, var(--l-border));
      color: #fff;
    }
  }

  /* Поверхность и рамка — как у окна профиля: непрозрачная, рамка тенью
     (border сдвинул бы пункты с сетки на пиксель). */
  &__dropdown {
    position: absolute;
    bottom: calc(100% + 12px);
    right: 0;
    width: max-content; /* по самому длинному пункту: «Добавить ярлык на рабочий стол» в одну строку */
    min-width: 280px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    background: var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
    overflow-y: auto;
    overscroll-behavior: contain;
    transform-origin: calc(100% - 32px) 100%;
    z-index: 40;

    /* Низкое окно: пункт 6 + плашка 30 + 6 вместо 8 + 32 + 8. */
    &.compact {
      gap: 3px;

      .game-menu__item {
        padding: 6px 8px;
      }

      .game-menu__item.separated {
        margin-top: 7px;

        &::before {
          top: -5px;
        }
      }

      .game-menu__icon {
        width: 30px;
        height: 30px;
      }
    }

    /* Совсем низкое окно: 3 + 28 + 3 — меню над кнопкой
       целиком влезает и в окно 1152×648 (Full HD 125 %). */
    &.tight {
      gap: 2px;

      .game-menu__item {
        padding: 3px 8px;
      }

      .game-menu__item.separated {
        margin-top: 6px;

        &::before {
          top: -5px;
        }
      }

      .game-menu__icon {
        width: 28px;
        height: 28px;
      }
    }
  }

  /* Пункт как карточка профиля: наведение подсвечивает рамку и фон, без
     подъёма. Высота та же, что была: 8 + плашка 32 + 8. */
  &__item {
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
    cursor: pointer;
    transition: background-color var(--l-fast), box-shadow var(--l-fast), color var(--l-fast);
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
    animation-delay: calc(var(--i, 0) * 24ms);
    /* Свой слой, как у поиска: при быстром наведении сглаженные углы рамки
       иначе оставляли по бокам не стёртые пиксели (видно при дробном
       масштабе экрана). */
    will-change: transform;

    &:hover:not(:disabled) {
      background: rgba(255, 255, 255, 0.05);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border));
    }

    &:active:not(:disabled) {
      background: rgba(255, 255, 255, 0.08);
    }

    &:focus-visible {
      outline: none;
      background: rgba(255, 255, 255, 0.05);
      box-shadow: inset 0 0 0 1px var(--l-accent);
    }

    &:disabled {
      color: var(--l-text-dim);
      cursor: not-allowed;
      opacity: 0.45;
    }

    /* Линия — тенью над пунктом: margin-top даёт ей место, высота пункта
       не меняется. */
    &.separated {
      position: relative;
      margin-top: 8px;

      &::before {
        content: "";
        position: absolute;
        left: 4px;
        right: 4px;
        top: -7px;
        height: 1px;
        background: var(--l-modal-border);
        pointer-events: none;
      }
    }

    &.danger:not(:disabled) {
      color: #ff7a7a;

      &:hover {
        background: rgba(255, 107, 107, 0.08);
        box-shadow: inset 0 0 0 1px rgba(255, 107, 107, 0.45);
      }

      &:focus-visible {
        box-shadow: inset 0 0 0 1px #ff6b6b;
      }
    }
  }

  &__label {
    flex: 1;
    min-width: 0;
  }

  /* Значок в плашке, как значки сводки профиля: на наведение плашка
     окрашивается в акцент. */
  &__icon {
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

  &__item:hover:not(:disabled) &__icon,
  &__item:focus-visible &__icon {
    background: var(--l-accent-soft);
    box-shadow: inset 0 0 0 1px transparent;
    color: var(--l-accent);
    transform: scale(1.06);
  }

  &__item.danger:not(:disabled) &__icon {
    background: rgba(255, 107, 107, 0.12);
    box-shadow: inset 0 0 0 1px transparent;
    color: #ff7a7a;
  }
}

.gm-enter-active {
  transition: opacity var(--l-dur) var(--l-ease), transform var(--l-dur) var(--l-ease);
}

.gm-leave-active {
  transition: opacity var(--l-dur-fast) var(--l-ease-in), transform var(--l-dur-fast) var(--l-ease-in);
}

.gm-enter-from,
.gm-leave-to {
  opacity: 0;
  transform: translateY(var(--l-rise)) scale(0.96);
}

.no-anim .gm-enter-active,
.no-anim .gm-leave-active {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .gm-enter-active,
  .gm-leave-active {
    transition: none;
  }
}
</style>
