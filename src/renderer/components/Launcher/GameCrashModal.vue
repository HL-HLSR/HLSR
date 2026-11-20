<template>
  <div class="crash" @mousedown.self="close">
    <div
      class="crash__window"
      role="alertdialog"
      aria-modal="true"
      :aria-label="$t('#L_CRM_TITLE')"
      ref="window"
    >
      <div class="crash__head">
        <i class="fas fa-exclamation-triangle crash__icon" aria-hidden="true"></i>
        <div class="crash__heading">
          <span class="crash__title">{{ $t("#L_CRM_TITLE") }}</span>
          <span v-if="subtitle" class="crash__game">{{ subtitle }}</span>
        </div>
      </div>

      <p class="crash__text">{{ $t(report.archive ? "#L_CRM_TEXT_ARCHIVE" : report.file ? "#L_CRM_TEXT_FILE" : "#L_CRM_TEXT_NO_FILE") }}</p>

      <!-- Путь целиком, с переносом: его переписывают или копируют (выделяется одним щелчком). -->
      <div v-if="report.file" class="crash__path" tabindex="0">{{ report.file }}</div>

      <span v-if="hintText" class="crash__note">
        <i class="fas fa-info-circle" aria-hidden="true"></i>
        {{ hintText }}
      </span>

      <div class="crash__actions">
        <button v-if="report.file" type="button" class="crash__btn" @click="$emit('open-folder', report.gameId)">
          <i class="fas fa-folder-open" aria-hidden="true"></i>
          {{ $t("#L_CRM_OPEN_FOLDER") }}
        </button>
        <button v-else type="button" class="crash__btn" @click="$emit('export-diagnostics')">
          <i class="fas fa-file-archive" aria-hidden="true"></i>
          {{ $t("#L_CR_EXPORT") }}
        </button>
        <button type="button" class="crash__btn" @click="$emit('open-discord')">
          <i class="fab fa-discord" aria-hidden="true"></i>
          Discord
        </button>
        <button type="button" class="crash__btn primary" ref="closeButton" @click="close">
          {{ $t("#L_CLOSE") }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";
import launcherData from "./launcher-data";

// Подсказки DEP для GoldSrc — те же, что в центре уведомлений.
const DEP_HINTS = {
  DEP_ALWAYS_ON: "#L_CR_DEP_ALWAYS_ON",
  DEP_EXEMPTION_FAILED: "#L_CR_DEP_EXEMPTION_FAILED",
};

// Окно о падении игры. report — событие game-crash-report: { gameId,
// launcherId, exitCode, hasDump, archive, file, hint }; file — архив сбоя,
// который собрала игра (archive: true), текстовый лог краша или дамп, null —
// ничего не нашлось (тогда просим файл диагностики).
export default {
  name: "GameCrashModal",
  emits: ["close", "open-folder", "open-discord", "export-diagnostics"],
  props: {
    report: {
      type: Object,
      required: true,
    },
  },
  computed: {
    subtitle() {
      const id = this.report.launcherId;
      const name = id ? launcherData.shortNames[id] : null;
      const code = this.report.exitCode;
      // Коды исключений Windows (0xC0000005) читаются только в шестнадцатеричном виде.
      const codeText = code == null ? null : code > 0xffff || code < 0 ? "0x" + (code >>> 0).toString(16).toUpperCase() : String(code);
      const parts = [name, codeText ? this.$t("#L_CRM_EXIT_CODE", codeText) : null].filter(Boolean);
      return parts.join(" · ");
    },
    hintText() {
      const key = DEP_HINTS[this.report.hint];
      return key ? this.$t(key) : null;
    },
  },
  methods: {
    close() {
      this.$emit("close");
    },
    onKeydown(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        return this.close();
      }

      if (e.key === "Tab") trapFocus(this.$refs.window, e);
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);

    this.$nextTick(() => {
      if (this.$refs.closeButton) this.$refs.closeButton.focus();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
.crash {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 70;

  &__window {
    width: min(560px, calc(100% - 80px));
    padding: 24px;
    background: var(--l-modal-bg);
    border: 1px solid var(--l-modal-border);
    border-radius: var(--l-radius);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);
    display: flex;
    flex-direction: column;
  }

  &__head {
    display: flex;
    align-items: flex-start;
    gap: 14px;
  }

  &__icon {
    flex: none;
    margin-top: 2px;
    color: #ffb37a;
    font-size: 26px;
  }

  &__heading {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  &__title {
    color: var(--l-text);
    font-size: 17px;
    font-weight: 700;
    line-height: 1.35;
  }

  &__game {
    color: var(--l-text-dim);
    font-size: 12.5px;
  }

  &__text {
    color: var(--l-text-dim);
    font-size: 13.5px;
    line-height: 1.55;
    margin: 16px 0 0;
  }

  &__path {
    margin-top: 10px;
    padding: 10px 12px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    color: var(--l-text);
    font-size: 12.5px;
    line-height: 1.5;
    word-break: break-all;
    user-select: all;
    cursor: text;
    outline: none;

    &:focus {
      border-color: var(--accent);
    }
  }

  &__note {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-top: 14px;
    color: #ffb37a;
    font-size: 12.5px;
    line-height: 1.45;
  }

  &__actions {
    display: flex;
    gap: 10px;
    margin-top: 22px;

    .primary {
      margin-left: auto;
    }
  }

  &__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 10px 18px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--l-fast), transform var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.1);
    }

    &:active {
      transform: scale(0.97);
    }

    &.primary {
      min-width: 110px;
      background: var(--accent);
      border-color: transparent;
      color: var(--accent-contrast-text);

      &:hover {
        filter: brightness(1.08);
      }
    }
  }
}
</style>
