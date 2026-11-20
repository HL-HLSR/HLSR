<template>
  <span class="gx-step" aria-hidden="true">
    <button type="button" class="gx-step__btn" tabindex="-1" @mousedown.prevent @click="step(1)"><i class="fas fa-chevron-up"></i></button>
    <button type="button" class="gx-step__btn" tabindex="-1" @mousedown.prevent @click="step(-1)"><i class="fas fa-chevron-down"></i></button>
  </span>
</template>

<script>
// Кнопки «больше/меньше» вместо системных стрелок числового поля: те
// рисуются поверх единицы измерения и не красятся под лаунчер. Ставится
// внутрь .gx-num рядом с <input type="number">. Шаг и пределы берёт сам
// input (stepUp/stepDown), а сохраняет обычный обработчик change поля.
// С клавиатуры поле по-прежнему меняют стрелки, поэтому кнопки скрыты от
// чтения с экрана и не встают в порядок Tab.
export default {
  name: 'GxNumStep',
  methods: {
    step(direction) {
      const input = this.$el.parentElement && this.$el.parentElement.querySelector('input[type="number"]');
      if (!input || input.disabled || input.readOnly) return;
      const before = input.value;
      // Пустое поле stepUp начинает с min или нуля.
      try {
        if (direction > 0) input.stepUp(); else input.stepDown();
      } catch (error) {
        return;
      }
      if (input.value === before) return;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    },
  },
};
</script>

<style lang="scss" scoped>
.gx-step {
  position: absolute;
  top: 4px;
  right: 4px;
  bottom: 4px;
  display: flex;
  flex-direction: column;
  width: 18px;
  overflow: hidden;
  border-radius: 6px;
  opacity: 0;
  transition: opacity var(--l-fast);

  &__btn {
    display: grid;
    flex: 1;
    place-items: center;
    min-height: 0;
    padding: 0;
    border: 0;
    background: rgba(255, 255, 255, 0.06);
    color: var(--l-text-dim);
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    + .gx-step__btn { border-top: 1px solid rgba(0, 0, 0, 0.25); }
    i { font-size: 8px; }
    &:hover { background: rgba(255, 255, 255, 0.14); color: var(--l-text); }
    &:active { background: var(--accent-soft); color: var(--accent); }
  }
}
// Кнопки видны, когда поле под курсором или в фокусе, как системные.
.gx-num:hover > .gx-step,
.gx-num:focus-within > .gx-step { opacity: 1; }
.gx-num:has(input:disabled) > .gx-step { display: none; }

@media (prefers-reduced-motion: reduce) {
  .gx-step, .gx-step__btn { transition: none; }
}
</style>
