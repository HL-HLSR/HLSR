<template>
  <div class="window-controls">
    <!-- Полный экран — левее «Свернуть» (просьба владельца 14.11). -->
    <button
      v-if="fullscreenButton"
      type="button"
      class="window-controls__btn"
      :title="$t(fullscreen ? '#L_FULLSCREEN_EXIT' : '#L_FULLSCREEN')"
      :aria-label="$t(fullscreen ? '#L_FULLSCREEN_EXIT' : '#L_FULLSCREEN')"
      :aria-pressed="fullscreen ? 'true' : 'false'"
      @click="toggleFullscreen"
    >
      <i class="fas" :class="fullscreen ? 'fa-compress' : 'fa-expand'" aria-hidden="true"></i>
    </button>
    <button
      type="button"
      class="window-controls__btn"
      :title="$t('#L_MINIMIZE')"
      :aria-label="$t('#L_MINIMIZE')"
      @click="minimize"
    >
      <i class="fas fa-minus" aria-hidden="true"></i>
    </button>
    <button
      type="button"
      class="window-controls__btn close"
      :title="$t('#L_CLOSE')"
      :aria-label="$t('#L_CLOSE')"
      @click="close"
    >
      <i class="fas fa-times" aria-hidden="true"></i>
    </button>
  </div>
</template>

<script>
const { ipcRenderer } = window.hlsrElectron;

// Полный экран — главный процесс (window-command "fullscreen", AppManager.js);
// из него окно выходит и само (свернули, спрятали в трей, сменили масштаб) —
// об этом приходит launcher-fullscreen-changed.
export default {
  name: "WindowControls",
  props: {
    fullscreenButton: { type: Boolean, default: true },
  },
  data() {
    return { fullscreen: false, switching: false };
  },
  watch: {
    // immediate: шапка пересоздаётся при смене языка, и класс от прошлой мог остаться
    // (окно успело выйти из полного экрана); верное состояние придёт из fullscreen-state.
    fullscreen: {
      immediate: true,
      handler(on) {
        document.documentElement.classList.toggle("window-fullscreen", on);
      },
    },
    // Кнопку выключили в настройках — полный экран без кнопки не оставляем.
    fullscreenButton(on) {
      if (!on && this.fullscreen) this.command("fullscreen-exit");
    },
  },
  mounted() {
    ipcRenderer.on("launcher-fullscreen-changed", this.onFullscreenChanged);
    window.addEventListener("keydown", this.onKeydown);
    // Смена языка пересоздаёт шапку, а окно может быть уже на весь экран.
    this.command("fullscreen-state");
  },
  beforeUnmount() {
    ipcRenderer.removeListener("launcher-fullscreen-changed", this.onFullscreenChanged);
    window.removeEventListener("keydown", this.onKeydown);
  },
  methods: {
    // F11 — и без кнопки в шапке. Ожидание клавиши для бинда (GameScripts, клавиатура)
    // ловит её раньше и гасит событие: тогда F11 уходит в бинд.
    onKeydown(event) {
      if (event.code !== "F11" || event.repeat || event.defaultPrevented) return;
      event.preventDefault();
      this.toggleFullscreen();
    },
    onFullscreenChanged(event, value) {
      this.fullscreen = value === true;
    },
    async command(name) {
      try {
        const result = await ipcRenderer.invoke("window-command", name);
        if (result && result.ok) this.fullscreen = result.fullscreen === true;
      } catch (e) {
        /* окна нет — состояние прежнее */
      }
    },
    // Плавно: окно гаснет (App.vue, window-switching), меняет размер и проявляется.
    // Без анимаций (настройка лаунчера или системная) — сразу.
    async toggleFullscreen() {
      if (this.switching) return;
      const root = document.documentElement;
      const animate = !document.querySelector(".no-anim") && !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      this.switching = true;
      try {
        if (animate) {
          root.classList.add("window-switching");
          await wait(130);
        }
        await this.command("fullscreen");
        // Новый размер и масштаб должны успеть нарисоваться до проявления.
        if (animate) await wait(70);
      } finally {
        root.classList.remove("window-switching");
        this.switching = false;
      }
    },
    minimize() {
      ipcRenderer.invoke("window-command", "minimize");
    },
    close() {
      ipcRenderer.invoke("window-command", "close");
    },
  },
};
</script>

<style lang="scss" scoped>
.window-controls {
  display: flex;
  /* Между значками — шаг шапки (launcher.css, --l-header-gap). */
  gap: calc(var(--l-header-gap, 32px) - var(--l-header-inset, 8px) * 2);
  -webkit-app-region: no-drag;

  &__btn {
    width: var(--l-header-control);
    height: var(--l-header-control);
    padding: 0;
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: rgba(255, 255, 255, 0.55);
    font-size: 17px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    i {
      width: 24px;
      line-height: 24px;
      text-align: center;
    }

    &:hover {
      background: rgba(255, 255, 255, 0.09);
      color: #fff;
    }

    &:active {
      background: rgba(255, 255, 255, 0.14);
    }

    /* Красный берётся из токенов кнопок (--btn-danger-*), а не из системной
       палитры Windows (#e81123): окно у лаунчера своё и без рамки, на Linux
       и Windows оно должно выглядеть одинаково, а не как «чужой» значок. */
    &.close:hover {
      background: var(--btn-danger-bg);
      color: #fff;
    }

    &.close:active {
      background: var(--btn-danger-pressed);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: -3px;
    }
  }
}
</style>
