<template>
  <div class="thanks" @mousedown.self="$emit('close')">
    <div class="thanks__window" role="dialog" aria-modal="true" :aria-label="$t('#L_THANKS_TITLE')" ref="window">
      <span class="thanks__title">{{ $t("#L_THANKS_TITLE") }}</span>
      <p class="thanks__text">{{ $t("#L_THANKS_TEXT") }}</p>
      <p v-if="notice.styles" class="thanks__text">{{ $t("#L_THANKS_STYLES") }}</p>
      <p v-if="until" class="thanks__text">{{ $t("#L_THANKS_UNTIL", until) }}</p>
      <button type="button" class="thanks__btn primary" ref="closeButton" @click="$emit('close')">{{ $t("#L_CLOSE") }}</button>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";

export default {
  name: "SupportThanksModal",
  emits: ["close"],
  props: {
    notice: { type: Object, required: true },
  },
  computed: {
    // Дата окончания подписки в том же виде, что в настройках ника (дд.мм.гггг).
    until() {
      if (this.notice.kind !== "subscription" || !this.notice.until) return "";
      const date = new Date(this.notice.until * 1000);
      if (isNaN(date.getTime())) return "";
      const pad = (n) => String(n).padStart(2, "0");
      return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
    },
  },
  methods: {
    onKeydown(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        return this.$emit("close");
      }
      if (e.key === "Tab") trapFocus(this.$refs.window, e);
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);
    this.$nextTick(() => this.$refs.closeButton && this.$refs.closeButton.focus());
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
.thanks {
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

  &__btn {
    margin-top: 22px;
    padding: 11px 18px;
    border: 1px solid transparent;
    border-radius: var(--l-radius-sm);
    background: var(--accent);
    color: var(--accent-contrast-text);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
    transition: filter var(--l-fast), transform var(--l-fast);

    &:hover {
      filter: brightness(1.08);
    }

    &:active {
      transform: scale(0.97);
    }
  }
}
</style>