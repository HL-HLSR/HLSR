<template>
  <div class="library-setup">
    <div class="library-setup__window" role="dialog" aria-modal="true" ref="window">
      <span class="library-setup__title">{{ $t("#L_FR_TITLE") }}</span>
      <p class="library-setup__text">{{ $t("#L_FR_TEXT") }}</p>

      <div class="library-setup__path">
        <span class="library-setup__value">{{ path }}</span>
        <span v-if="freeSpaceGb" class="library-setup__space">{{
          $t("#L_FR_FREE_SPACE", freeSpaceGb)
        }}</span>
      </div>

      <span v-if="error" class="library-setup__error">{{ error }}</span>
      <span v-if="hasCyrillic" class="library-setup__warning">{{
        $t("#UI_FOUND_CYRILLIC")
      }}</span>

      <div class="library-setup__actions">
        <button
          type="button"
          class="library-setup__btn"
          ref="chooseButton"
          @click="choose"
        >
          {{ $t("#L_FR_CHOOSE") }}
        </button>
        <button
          type="button"
          class="library-setup__btn primary"
          :disabled="!!error"
          @click="confirm"
        >
          {{ $t("#L_FR_CONFIRM") }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
const { ipcRenderer } = window.hlsrElectron;
import { trapFocus } from "./focus-trap";

export default {
  name: "LibrarySetupModal",
  emits: ["confirmed"],
  props: {
    initialPath: {
      type: String,
      required: true,
    },
  },
  data() {
    return {
      path: this.initialPath,
      freeSpaceMb: 0,
      hasCyrillic: false,
      error: null,
    };
  },
  computed: {
    freeSpaceGb() {
      let mbytes = Number(this.freeSpaceMb);
      if (!isFinite(mbytes) || mbytes <= 0) return null;
      return (mbytes / 1024).toFixed(1).replace(".", ",");
    },
  },
  methods: {
    async validate(target) {
      let check = await ipcRenderer.invoke("validate-install-path", target);

      if (!check.ok) {
        this.error =
          check.error === "no-access"
            ? this.$t("#L_S_PATH_NO_ACCESS")
            : this.$t("#L_S_PATH_INVALID");
        return false;
      }

      this.error = null;
      this.path = check.path;
      this.freeSpaceMb = Number(check.freeSpaceMb) || 0;
      this.hasCyrillic = !!check.hasCyrillic;
      return true;
    },
    async choose() {
      let selected = await ipcRenderer.invoke("select-install-folder", this.path);
      if (!selected) return;

      await this.validate(selected);
    },
    async confirm() {
      let result = await ipcRenderer.invoke("set-library-path", this.path);

      if (!result.ok) {
        this.error =
          result.error === "no-access"
            ? this.$t("#L_S_PATH_NO_ACCESS")
            : this.$t(result.error === "busy" ? "#L_S_PATH_BUSY" : "#L_S_PATH_INVALID");
        return;
      }

      this.$emit("confirmed", result.path);
    },
    onKeydown(e) {
      if (e.key === "Tab") trapFocus(this.$refs.window, e);
    },
  },
  mounted() {
    document.addEventListener("keydown", this.onKeydown);
    this.validate(this.path);
    this.$nextTick(() => this.$refs.chooseButton.focus());
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
  },
};
</script>

<style lang="scss" scoped>
.library-setup {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;

  z-index: 15;

  &__window {
    width: min(520px, calc(100% - 80px));
    padding: 26px;
    background: var(--l-modal-bg);
    border: 1px solid var(--l-modal-border);
    border-radius: var(--l-radius);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);
    display: flex;
    flex-direction: column;
  }

  &__title {
    color: var(--l-text);
    font-size: 16px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.09em;
  }

  &__text {
    color: var(--l-text-dim);
    font-size: 13.5px;
    line-height: 1.55;
    margin: 12px 0 18px;
  }

  &__path {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 13px 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
  }

  &__value {
    color: var(--l-text);
    font-size: 13.5px;
    font-weight: 500;
    word-break: break-all;
  }

  &__space {
    color: var(--l-text-dim);
    font-size: 12px;
    white-space: nowrap;
  }

  &__error {
    margin-top: 10px;
    color: #ff6b78;
    font-size: 12.5px;
    line-height: 1.4;
  }

  &__warning {
    margin-top: 10px;
    color: #ffb37a;
    font-size: 12.5px;
    line-height: 1.4;
  }

  &__actions {
    display: flex;
    gap: 10px;
    margin-top: 22px;
  }

  &__btn {
    flex: 1;
    padding: 11px 18px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast), transform var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.1);
    }

    &:active {
      transform: scale(0.97);
    }

    &.primary {
      background: var(--accent);
      border-color: transparent;
      color: var(--accent-contrast-text);

      &:hover {
        background: var(--accent-hover);
      }

      &:disabled {
        opacity: 0.5;
        cursor: default;
        background: var(--accent);
      }
    }
  }
}
</style>
