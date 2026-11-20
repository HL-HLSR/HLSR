<template>

  <transition-group tag="div" class="toasts" name="toast" aria-live="polite" @before-leave="rememberHeight">
    <div
      v-for="toast in toasts"
      :key="toast.id"
      class="toasts__item"
      :class="'is-' + toneOf(toast)"
      :style="{ '--toast-life': (toast.duration || 2800) + 'ms' }"
      @click="$emit('dismiss', toast.id)"
    >
      <span class="toasts__icon" aria-hidden="true"><i :class="toast.icon"></i></span>
      <span class="toasts__text">{{ toast.text }}</span>
      <i class="fas fa-times toasts__close" aria-hidden="true"></i>
      <span class="toasts__life" aria-hidden="true"></span>
    </div>
  </transition-group>
</template>

<script>
// Тон тоста — по значку, который передаёт notify(): так все вызовы
// оболочки окрашиваются без правки каждого из них.
const TONES = [
  { tone: "success", icons: ["fa-check", "fa-clipboard-check", "fa-check-circle", "fa-file-import"] },
  { tone: "warning", icons: ["fa-exclamation-triangle", "fa-lock", "fa-clock"] },
  { tone: "danger", icons: ["fa-exclamation-circle", "fa-times-circle"] },
];

export default {
  name: "ToastContainer",
  emits: ["dismiss"],
  props: {
    toasts: {
      type: Array,
      required: true,
    },
  },
  methods: {
    // Высота нужна анимации скрытия: тост схлопывается от своей высоты
    // до нуля, и соседи съезжают на его место вместе с ним.
    rememberHeight(el) {
      el.style.setProperty("--toast-h", el.offsetHeight + "px");
    },
    toneOf(toast) {
      const icons = String(toast.icon || "").split(/\s+/);
      const found = TONES.find((entry) => entry.icons.some((icon) => icons.includes(icon)));
      return found ? found.tone : "info";
    },
  },
};
</script>

<style lang="scss" scoped>
.toasts {
  position: absolute;
  right: 24px;
  /* Выше панели настроек/версии сборки, чтобы не перекрывать её */
  bottom: calc(var(--l-bottombar-h) + 108px);
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  z-index: 60;
  pointer-events: none;

  /* Карточка в стиле окна профиля: непрозрачная поверхность, рамка тенью,
     значок в плашке цвета сообщения. Цвет задаёт --toast-tint. */
  &__item {
    --toast-tint: var(--l-accent);
    position: relative;
    pointer-events: all;
    display: flex;
    align-items: center;
    gap: 12px;
    width: 344px;
    min-height: 56px;
    padding: 12px 16px 12px 12px;
    background:
      radial-gradient(120% 140% at 0% 0%, color-mix(in srgb, var(--toast-tint) 12%, transparent) 0%, transparent 60%),
      var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--toast-tint) 28%, var(--l-modal-border)), var(--l-shadow-pop);
    color: var(--l-text);
    font-size: 13px;
    cursor: pointer;
    overflow: hidden;
    transition: transform var(--l-med), opacity var(--l-med), box-shadow var(--l-fast);

    &.is-success { --toast-tint: #4ad07a; }
    &.is-warning { --toast-tint: #ffb37a; }
    &.is-danger { --toast-tint: #ff7a7a; }

    &:hover {
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--toast-tint) 50%, var(--l-modal-border)), var(--l-shadow-pop);
    }
  }

  &__icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--l-radius-sm);
    background: color-mix(in srgb, var(--toast-tint) 16%, transparent);
    color: var(--toast-tint);
    font-size: 14px;
    animation: toast-icon var(--l-dur-slow) var(--l-ease-spring) both;
    animation-delay: 60ms;

    > i { line-height: 1; }
  }

  &__text {
    flex: 1;
    min-width: 0;
    line-height: 20px;
    word-break: break-word;
  }

  /* Крестик проявляется при наведении: щелчок по тосту и так его
     закрывает, значок только подсказывает это. */
  &__close {
    flex: none;
    width: 16px;
    color: var(--l-text-faint);
    font-size: 11px;
    text-align: center;
    opacity: 0;
    transition: opacity var(--l-fast), color var(--l-fast);
  }

  &__item:hover &__close {
    opacity: 1;
    color: var(--l-text);
  }

  /* Сколько тосту осталось: полоска тает за время его жизни. */
  &__life {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background: var(--toast-tint);
    opacity: 0.6;
    transform-origin: 0 50%;
    animation: toast-life var(--toast-life) linear both;
  }
}

@keyframes toast-life {
  from { transform: scaleX(1); }
  to { transform: scaleX(0); }
}

@keyframes toast-icon {
  from { opacity: 0; transform: scale(0.6); }
}

.toast-enter-active {
  transition: transform var(--l-dur-slow) var(--l-ease-spring), opacity var(--l-dur) var(--l-ease);
}

.toast-enter-from {
  opacity: 0;
  transform: translateX(32px) scale(0.96);
}

/* Соседи съезжают на освободившееся место плавно. */
.toast-move {
  transition: transform var(--l-dur) var(--l-ease);
}

/* Скрытие: тост уезжает вправо и гаснет, потом схлопывается по высоте
   вместе с отступом до соседа. Он остаётся в потоке, поэтому соседи не
   прыгают, а плавно занимают место. Селектор сильнее .toast-move: иначе
   её transition отменил бы анимацию. */
.toasts__item.toast-leave-active {
  pointer-events: none;
  transition: none;
  animation: toast-out 420ms var(--l-ease-in) both;
}

@keyframes toast-out {
  0% {
    height: var(--toast-h);
  }
  55% {
    opacity: 0;
    transform: translateX(56px) scale(0.96);
    height: var(--toast-h);
    margin-bottom: 0;
  }
  100% {
    opacity: 0;
    transform: translateX(56px) scale(0.96);
    height: 0;
    min-height: 0;
    padding-top: 0;
    padding-bottom: 0;
    margin-bottom: -8px;
  }
}

.no-anim .toasts__item.toast-leave-active {
  animation-duration: 1ms;
}

@media (prefers-reduced-motion: reduce) {
  .toasts__item.toast-leave-active {
    animation-duration: 1ms;
  }
}
</style>
