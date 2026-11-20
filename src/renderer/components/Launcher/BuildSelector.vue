<template>
  <div class="build-panel">


    <UiDropdown
      :items="items"
      :value="effectiveValue"
      :heading="$t('#L_BUILD_VERSION')"
      direction="up"
      align="right"
      @select="$emit('select-build', $event.id)"
    >
      <template #toggle="{ open }">

        <span
          class="build-panel__toggle"
          :class="{ open: open }"
          :title="$t('#L_BUILD_VERSION')"
        >
          <span class="build-panel__labels">
            <span class="build-panel__caption">{{
              $t("#L_BUILD_VERSION")
            }}</span>
            <span class="build-panel__value" :title="effectiveValue">{{ effectiveValue }}</span>
          </span>
          <span class="build-panel__chev" aria-hidden="true"><i class="fas fa-chevron-up"></i></span>
        </span>
      </template>

      <template #item="{ item }">
        <span class="build-panel__item-icon" :class="{ soon: item.disabled }" aria-hidden="true">
          <i class="fas" :class="item.disabled ? 'fa-hourglass-half' : 'fa-code-branch'"></i>
        </span>
        <span class="build-panel__item-label">{{ item.label }}</span>
      </template>
    </UiDropdown>
  </div>
</template>

<script>
import UiDropdown from "./UiDropdown";

export default {
  name: "BuildSelector",
  components: { UiDropdown },
  emits: ["select-build"],
  props: {
    builds: {
      type: Array,
      required: true,
    },
    // Сборки, которые появятся позже: видны в списке, но не выбираются.
    upcoming: {
      type: Array,
      default: () => [],
    },
    value: {
      type: String,
      required: true,
    },
    // Подписи справа у пунктов по имени сборки: размер нескачанного эпизода.
    badges: {
      type: Object,
      default: () => ({}),
    },
  },
  computed: {
    items() {
      const ready = this.builds.map((name) => ({ id: name, label: name, badge: this.badges[name] }));
      const soon = this.upcoming.map((name, index) => ({
        id: "upcoming:" + name,
        label: name,
        disabled: true,
        separated: index === 0,
        badge: this.$t("#L_BUILD_SOON"),
        hint: name + " — " + this.$t("#L_BUILD_SOON_HINT"),
      }));
      return ready.concat(soon);
    },
    effectiveValue() {
      return this.builds.indexOf(this.value) !== -1 ? this.value : this.builds[0];
    },
  },
};
</script>

<style lang="scss" scoped>
/* Ширина совпадает с «Играть»; полное название доступно в меню и подсказке.
   Карточка — как блоки окна профиля: рамка тенью, при наведении на любую
   из половин рамка берёт оттенок акцента. */
.build-panel {
  display: flex;
  align-items: stretch;
  width: var(--l-play-w);
  background: var(--l-surface-solid);
  border-radius: var(--l-radius);
  box-shadow: inset 0 0 0 1px var(--l-border);
  overflow: visible;
  transition: box-shadow var(--l-fast);

  &:hover,
  &:focus-within {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 35%, var(--l-border));
  }

  /* Кнопка выбора тянется на всю оставшуюся ширину карточки, стрелка
     уходит к правому краю */
  :deep(.ui-dropdown) {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
  }

  :deep(.ui-dropdown__toggle) {
    flex: 1;
    min-width: 0;
    border-radius: var(--l-radius);
  }

  :deep(.ui-dropdown__toggle:focus-visible) {
    outline: none;
  }

  :deep(.ui-dropdown__toggle:focus-visible .build-panel__toggle) {
    box-shadow: inset 0 0 0 1px var(--l-accent);
  }

  &__toggle {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-width: 0;
    height: var(--l-build-h);
    padding: 0 12px 0 16px;
    border-radius: var(--l-radius);
    transition: background-color var(--l-fast), box-shadow var(--l-fast);
  }

  &__toggle:hover,
  &__toggle.open {
    background: rgba(255, 255, 255, 0.04);
  }

  &__labels {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  &__caption {
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    transition: color var(--l-fast);
  }

  &__toggle:hover &__caption,
  &__toggle.open &__caption {
    color: var(--l-accent);
  }

  &__value {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--l-text);
    font-size: 15px;
    font-weight: 700;
    line-height: 20px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  /* Стрелка — в плашке: меню открывается вверх,
     открытое — стрелка разворачивается вниз. */
  &__chev {
    flex: none;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 10px;
    transition: background-color var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);

    > i {
      line-height: 1;
      transition: transform var(--l-dur) var(--l-ease);
    }
  }

  &__toggle:hover &__chev,
  &__toggle.open &__chev {
    background: var(--l-accent-soft);
    box-shadow: inset 0 0 0 1px transparent;
    color: var(--l-accent);
  }

  &__toggle.open &__chev > i {
    transform: rotate(180deg);
  }

  /* Пункты меню: значок в плашке, как у пунктов меню игры. */
  &__item-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    margin-left: -4px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 12px;
    transition: background-color var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);

    > i { line-height: 1; }

    &.soon {
      background: transparent;
      color: var(--l-text-faint);
    }
  }

  &__item-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
}

/* Меню живёт в отдельном слое, но пункты несут метку этого компонента. */
:global(.ui-dropdown__item:hover:not(:disabled) .build-panel__item-icon),
:global(.ui-dropdown__item:focus-visible .build-panel__item-icon),
:global(.ui-dropdown__item.selected .build-panel__item-icon) {
  background: var(--l-accent-soft);
  box-shadow: inset 0 0 0 1px transparent;
  color: var(--l-accent);
}

@media (prefers-reduced-motion: reduce) {
  .build-panel__chev > i {
    transition: none;
  }
}
</style>
