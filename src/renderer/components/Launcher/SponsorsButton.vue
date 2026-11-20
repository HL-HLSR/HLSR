<template>
  <div class="sponsors">
    <!-- Сердечко как у кнопки Sponsor на GitHub: розовый контур, при
         наведении бьётся и заливается. Открывает окно «Спонсоры»
         (SponsorsModal, показывает LauncherShell). -->
    <button
      type="button"
      class="sponsors__btn"
      :class="{ open: active }"
      :title="$t('#L_S_SPONSORS')"
      :aria-label="$t('#L_S_SPONSORS')"
      aria-haspopup="dialog"
      @click="$emit('open')"
    >
      <span class="sponsors__heart" aria-hidden="true">
        <i class="far fa-heart sponsors__heart-outline"></i>
        <i class="fas fa-heart sponsors__heart-fill"></i>
      </span>
    </button>
  </div>
</template>

<script>
export default {
  name: "SponsorsButton",
  emits: ["open"],
  props: {
    active: { type: Boolean, default: false },
  },
};
</script>

<style lang="scss" scoped>
/* Розовый GitHub Sponsors (#db61a2). */
.sponsors {
  --sponsor: #db61a2;

  position: relative;
  -webkit-app-region: no-drag;

  &__btn {
    display: grid;
    place-items: center;
    width: var(--l-header-control);
    height: var(--l-header-control);
    padding: 0;
    background: transparent;
    border: none;
    border-radius: var(--l-radius-sm);
    color: var(--sponsor);
    font-size: 22px;
    cursor: pointer;

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: 0;
    }
  }

  &__heart {
    position: relative;
    display: block;
    width: 24px;
    height: 24px;
    line-height: 24px;
    text-align: center;

    > i {
      position: absolute;
      inset: 0;
      line-height: inherit;
      transition: opacity var(--l-fast) var(--l-ease), transform var(--l-base) var(--l-ease-spring);
    }
  }

  &__heart-fill {
    opacity: 0;
    transform: scale(0.6);
  }

  &__btn:hover &__heart,
  &__btn.open &__heart {
    animation: sponsors-beat 1.1s var(--l-ease) infinite;
  }

  &__btn:hover &__heart-fill,
  &__btn.open &__heart-fill {
    opacity: 1;
    transform: scale(1);
  }

  &__btn:active &__heart {
    transform: scale(0.88);
  }
}

@keyframes sponsors-beat {
  0%, 40%, 100% { transform: scale(1); }
  14% { transform: scale(1.18); }
  28% { transform: scale(1.04); }
}

.no-anim .sponsors__heart {
  animation: none !important;
}
</style>
