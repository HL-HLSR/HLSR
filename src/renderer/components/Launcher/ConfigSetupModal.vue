<template>
  <div class="settings-modal" @mousedown.self="$emit('close')">
    <div
      class="settings-modal__window cfgsetup__window"
      role="dialog"
      aria-modal="true"
      :aria-label="$t('#L_CFG_SETUP_TITLE')"
      ref="window"
    >
      <header class="settings-modal__header">
        <span class="settings-modal__title">{{ $t("#L_CFG_SETUP_TITLE") }}</span>
        <button
          type="button"
          class="settings-modal__close"
          :title="$t('#L_CLOSE')"
          :aria-label="$t('#L_CLOSE')"
          @click="$emit('close')"
        >
          <i class="fas fa-times"></i>
        </button>
      </header>

      <div class="cfgsetup__body">
        <p class="cfgsetup__text">{{ $t("#L_CFG_SETUP_TEXT") }}</p>
        <div class="cfgsetup__actions">
          <button type="button" class="settings-btn primary" data-choice="scratch" @click="$emit('choose', 'scratch')">
            <i class="fas fa-magic" aria-hidden="true"></i>
            {{ $t("#L_CFG_SETUP_SCRATCH") }}
          </button>
          <button type="button" class="settings-btn" data-choice="own" @click="$emit('choose', 'own')">
            <i class="fas fa-file-import" aria-hidden="true"></i>
            {{ $t("#L_CFG_SETUP_OWN") }}
          </button>
          <button type="button" class="settings-btn" data-choice="skip" @click="$emit('choose', 'skip')">
            <i class="fas fa-forward" aria-hidden="true"></i>
            {{ $t("#L_CFG_SETUP_SKIP") }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script>
import uiState from "./ui-state";
import { trapFocus } from "./focus-trap";

// Опрос при первом запуске игры без конфига. «Закрыть» и Esc отменяют запуск, ничего не отмечая.
export default {
  name: "ConfigSetupModal",
  emits: ["choose", "close"],
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
    // Фокус на «Пропустить»: Enter сразу после «Играть» запускает игру, а не уводит в «С нуля».
    this.$nextTick(() => {
      const btn = this.$refs.window.querySelector('.cfgsetup__actions [data-choice="skip"]');
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
  }
}

.cfgsetup {
  &__window {
    width: min(520px, calc(100% - 80px));
  }

  &__body {
    padding: 20px 24px 22px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  &__text {
    margin: 0;
    color: var(--l-text);
    font-size: 14px;
    line-height: 1.5;
  }

  &__actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }
}

.settings-btn {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 9px 16px;
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--l-fast);

  i {
    color: var(--l-text-dim);
  }

  &:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  &.primary {
    background: rgba(255, 255, 255, 0.14);
  }
}

// Без анимаций (настройка лаунчера или системная) окно не переходит плавно.
.no-anim .settings-btn,
.no-anim .settings-modal__close {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .settings-btn,
  .settings-modal__close {
    transition: none;
  }
}
</style>
