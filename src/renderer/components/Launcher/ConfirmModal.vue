<template>
  <div class="confirm" @mousedown.self="cancel">
    <div
      class="confirm__window"
      role="alertdialog"
      aria-modal="true"
      :aria-label="title"
      ref="window"
    >
      <span class="confirm__title">{{ title }}</span>
      <p class="confirm__text">{{ text }}</p>

      <ul v-if="items.length" class="confirm__items">
        <li v-for="item in items" :key="item" class="confirm__item">{{ item }}</li>
      </ul>

      <slot></slot>

      <div v-if="choices.length" class="confirm__choices" role="radiogroup" :aria-label="title">
        <button
          v-for="option in choices"
          :key="option.id"
          type="button"
          role="radio"
          class="confirm__choice"
          :class="{ active: option.id === choice }"
          :aria-checked="option.id === choice ? 'true' : 'false'"
          @click="choice = option.id"
          @dblclick="choice = option.id; accept()"
        >
          <i :class="option.id === choice ? 'fas fa-dot-circle' : 'far fa-circle'" aria-hidden="true"></i>
          <span>{{ option.label }}</span>
        </button>
      </div>

      <span v-if="note" class="confirm__note">
        <i class="fas fa-exclamation-triangle" aria-hidden="true"></i>
        {{ note }}
      </span>

      <div class="confirm__actions">
        <button type="button" class="confirm__btn" ref="cancelButton" @click="cancel">
          {{ cancelLabel || $t("#L_CANCEL") }}
        </button>
        <button
          type="button"
          class="confirm__btn primary"
          :class="{ danger: danger }"
          :disabled="confirmDisabled"
          @click="accept"
        >
          {{ confirmLabel || $t("#L_CONFIRM") }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";

export default {
  name: "ConfirmModal",
  emits: ["resolve"],
  props: {
    title: {
      type: String,
      required: true,
    },
    text: {
      type: String,
      required: true,
    },
    items: {
      type: Array,
      default: () => [],
    },
    note: {
      type: String,
      default: null,
    },
    confirmLabel: {
      type: String,
      default: null,
    },
    cancelLabel: {
      type: String,
      default: null,
    },
    danger: {
      type: Boolean,
      default: false,
    },
    confirmDisabled: {
      type: Boolean,
      default: false,
    },
    // Варианты [{ id, label }]: тогда окно отвечает id выбранного, а не true.
    choices: {
      type: Array,
      default: () => [],
    },
    defaultChoice: {
      type: String,
      default: null,
    },
  },
  data() {
    const ids = this.choices.map((option) => option.id);
    return { choice: ids.includes(this.defaultChoice) ? this.defaultChoice : ids[0] || null };
  },
  methods: {
    accept() {
      this.$emit("resolve", this.choices.length ? this.choice : true);
    },
    cancel() {
      this.$emit("resolve", false);
    },
    onKeydown(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        return this.cancel();
      }

      if (e.key === "Tab") trapFocus(this.$refs.window, e);
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);

    this.$nextTick(() => {
      if (this.$refs.cancelButton) this.$refs.cancelButton.focus();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
.confirm {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 70;

  &__window {
    width: min(460px, calc(100% - 80px));
    padding: 24px;
    background: var(--l-modal-bg);
    border: 1px solid var(--l-modal-border);
    border-radius: var(--l-radius);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);
    display: flex;
    flex-direction: column;
  }

  &__title {
    color: var(--l-text);
    font-size: 15px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.09em;
  }

  &__text {
    color: var(--l-text-dim);
    font-size: 13.5px;
    line-height: 1.55;
    margin: 12px 0 0;
  }

  &__items {
    list-style: none;
    margin: 14px 0 0;
    padding: 12px 14px;
    max-height: 132px;
    overflow-y: auto;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
  }

  &__item {
    color: var(--l-text);
    font-size: 12.5px;
    line-height: 1.6;
    // длинный путь переносится где угодно, обычные слова — только целиком
    overflow-wrap: anywhere;
  }

  &__choices {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 14px;
  }

  &__choice {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.04);
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 13.5px;
    text-align: left;
    cursor: pointer;
    transition: background var(--l-fast), border-color var(--l-fast), color var(--l-fast);

    i {
      font-size: 13px;
    }

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      color: var(--l-text);
    }

    &.active {
      border-color: var(--accent);
      color: var(--l-text);

      i {
        color: var(--accent);
      }
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
        filter: brightness(1.08);
      }
    }

    &:disabled {
      opacity: 0.5;
      cursor: default;
      transform: none;
      filter: none;
    }

    &.danger {
      background: #c0392f;
      color: #fff;
    }
  }
}
</style>
