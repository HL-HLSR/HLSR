<template>
  <div class="settings-modal" @mousedown.self="$emit('close')">
    <div
      class="settings-modal__window game-notes__window"
      role="dialog"
      aria-modal="true"
      :aria-label="title"
      ref="window"
    >
      <header class="settings-modal__header">
        <span class="settings-modal__title">{{ title }}</span>
        <button
          type="button"
          class="settings-modal__close"
          :title="$t('#L_CLOSE')"
          :aria-label="$t('#L_CLOSE')"
          @click="$emit('close')"
        >
          <i class="fas fa-times" aria-hidden="true"></i>
        </button>
      </header>

      <div class="game-notes__body">

        <ul v-if="items.length" class="game-notes__items">
          <li v-for="(item, index) in items" :key="index">{{ item }}</li>
        </ul>
        <div v-else class="game-notes__empty">
          <i class="far fa-file-alt" aria-hidden="true"></i>
          <span>{{ $t("#L_GN_EMPTY") }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<script>
import uiState from "./ui-state";
import { trapFocus } from "./focus-trap";

export default {
  name: "GameNotesModal",
  emits: ["close"],
  props: {
    gameTitle: {
      type: String,
      required: true,
    },
    version: {
      type: String,
      required: true,
    },
    notes: {
      type: Object,
      default: null,
    },
  },
  computed: {
    title() {
      return this.$t("#L_GN_TITLE", `${this.gameTitle} ${this.version}`);
    },
    items() {
      if (!this.notes || !Array.isArray(this.notes.items)) return [];
      if (this.notes.version !== this.version) return [];

      // Предел — MAX_ITEM_LENGTH из release-notes.js: там пункт уже обрезан по слову с «…»,
      // а прежний срез на 200 рвал его на полуслове.
      return this.notes.items
        .filter((item) => typeof item === "string" && item.trim())
        .map((item) => item.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim().slice(0, 300))
        .filter(Boolean)
        .slice(0, 20);
    },
  },
  methods: {
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
    document.addEventListener("keydown", this.onKeydown);
    this.$nextTick(() => {
      const btn = this.$refs.window.querySelector("button");
      if (btn) btn.focus();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
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
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border: 1px solid var(--l-modal-border);
    border-radius: var(--l-radius);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }

  &__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 18px 16px 24px;
    border-bottom: 1px solid var(--l-border);
    flex: none;
  }

  &__title {
    color: var(--l-text);
    font-size: 15px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.09em;
  }

  &__close {
    width: 36px;
    height: 36px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--l-text-dim);
    font-size: 17px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      color: #fff;
    }

    &:active {
      background: rgba(255, 255, 255, 0.12);
    }
  }
}

.game-notes {
  &__window {
    width: min(560px, calc(100% - 80px));
    max-height: min(520px, calc(100% - 80px));
  }

  &__body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 20px 24px 24px;
  }

  &__items {
    margin: 0;
    padding-left: 18px;
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.6;

    li {
      margin-bottom: 4px;
    }
  }

  &__empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 34px 0 22px;
    color: var(--l-text-faint);
    font-size: 13px;

    i {
      font-size: 26px;
    }
  }
}
</style>
