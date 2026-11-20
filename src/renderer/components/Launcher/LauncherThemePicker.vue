<template>
  <div class="pz-themes__grid" role="radiogroup" :aria-label="$t('#L_PZ_THEMES')">
    <button
      v-for="theme in themes"
      :key="theme.id"
      type="button"
      role="radio"
      class="pz-theme"
      :class="{ active: theme.id === currentTheme }"
      :aria-checked="theme.id === currentTheme ? 'true' : 'false'"
      :style="{ '--sw-bg': theme.swatch[0], '--sw-surface': theme.swatch[1], '--sw-accent': swatchAccent(theme) }"
      @click="$emit('theme', theme.id)"
    >
      <!-- Миниатюра лаунчера в цветах темы: шапка, колонка значков,
           заголовок игры и кнопка «Играть». -->
      <span class="pz-theme__mock" aria-hidden="true">
        <span class="pz-theme__bar">
          <span class="pz-theme__logo"></span>
          <span class="pz-theme__tab is-on"></span>
          <span class="pz-theme__tab"></span>
        </span>
        <span class="pz-theme__rail">
          <span></span><span></span><span></span>
        </span>
        <span class="pz-theme__hero">
          <span class="pz-theme__line is-wide"></span>
          <span class="pz-theme__line"></span>
        </span>
        <span class="pz-theme__play"></span>
      </span>
      <span class="pz-theme__text">
        <span class="pz-theme__name">
          {{ $t(theme.title) }}
          <i v-if="theme.id === currentTheme" class="fas fa-check" aria-hidden="true"></i>
        </span>
        <span class="pz-theme__hint">{{ $t(theme.hint) }}</span>
      </span>
    </button>
  </div>
  <!-- Варианты акцента — только у выбранной темы, где они есть («Бумага»). -->
  <div v-if="accents" class="pz-accents" role="radiogroup" :aria-label="$t('#L_THEME_PAPER_ACCENT')">
    <span class="pz-accents__label" aria-hidden="true">{{ $t("#L_THEME_PAPER_ACCENT") }}</span>
    <button
      v-for="item in accents"
      :key="item.id"
      type="button"
      role="radio"
      class="pz-accent"
      :class="{ active: item.id === accentId }"
      :aria-checked="item.id === accentId ? 'true' : 'false'"
      :style="{ '--sw-accent': item.color }"
      @click="$emit('theme', currentTheme, item.id)"
    >
      <span class="pz-accent__dot" aria-hidden="true"></span>{{ $t(item.title) }}
    </button>
  </div>
</template>

<script>
import { THEMES } from "./launcher-themes";

// Сетка тем лаунчера и под ней варианты акцента выбранной темы. Выбор
// применяется сразу, сохранять тут нечего: событие theme(id, accent).
export default {
  name: "LauncherThemePicker",
  emits: ["theme"],
  props: {
    currentTheme: { type: String, required: true },
    currentAccent: { type: String, default: "" },
  },
  data() {
    return { themes: THEMES };
  },
  computed: {
    accents() {
      const theme = THEMES.find((item) => item.id === this.currentTheme);
      return (theme && theme.accents) || null;
    },
    accentId() {
      if (!this.accents) return "";
      const found = this.accents.find((item) => item.id === this.currentAccent);
      return found ? found.id : this.accents[0].id;
    },
  },
  methods: {
    // Карточка темы с вариантами показывает выбранный акцент.
    swatchAccent(theme) {
      if (!theme.accents || theme.id !== this.currentTheme) return theme.swatch[2];
      return this.accents.find((item) => item.id === this.accentId).color;
    },
  },
};
</script>

<style lang="scss" scoped>
.pz-themes__grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 16px;
  padding-bottom: 8px;
}

.pz-theme {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  border: none;
  border-radius: var(--l-radius);
  background: var(--l-modal-bg);
  /* Прозрачная тень снаружи — запас области перерисовки: при масштабе
     125 % рамка на скруглённых углах иначе оставляла точки после наведения. */
  box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 0 0 1px transparent;
  color: var(--l-text);
  font: inherit;
  text-align: left;
  cursor: pointer;
  /* Карточки стоят на месте: наведение только подсвечивает рамку, выбранная
     обведена цветом своей темы. */
  transition: box-shadow var(--l-fast);

  &:hover {
    box-shadow: inset 0 0 0 1px var(--l-border-strong), 0 0 0 1px transparent;
  }

  &.active {
    box-shadow: inset 0 0 0 2px var(--sw-accent), 0 0 0 1px transparent;
  }

  &__mock {
    position: relative;
    display: block;
    height: 128px;
    border-radius: var(--l-radius-sm);
    overflow: hidden;
    background:
      radial-gradient(120% 90% at 85% 100%, color-mix(in srgb, var(--sw-accent) 22%, transparent), transparent 60%),
      var(--sw-bg);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  &__bar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 20px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    background: color-mix(in srgb, var(--sw-surface) 80%, transparent);
  }

  &__logo {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    box-shadow: 0 0 0 1.5px #fff;
    margin-right: 6px;
  }

  &__tab {
    width: 22px;
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.28);

    &.is-on { background: var(--sw-accent); }
  }

  &__rail {
    position: absolute;
    top: 20px;
    bottom: 0;
    left: 0;
    width: 20px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding-top: 8px;
    background: color-mix(in srgb, var(--sw-surface) 60%, transparent);

    span {
      width: 8px;
      height: 8px;
      border-radius: 50%;
      background: rgba(255, 255, 255, 0.22);

      &:first-child { background: var(--sw-accent); }
    }
  }

  &__hero {
    position: absolute;
    left: 32px;
    top: 44px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  &__line {
    width: 56px;
    height: 5px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.5);

    &.is-wide { width: 96px; height: 9px; background: rgba(255, 255, 255, 0.85); }
  }

  &__play {
    position: absolute;
    right: 12px;
    bottom: 12px;
    width: 64px;
    height: 18px;
    border-radius: 4px;
    background: var(--sw-accent);
  }

  &__text {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0 4px 4px;
  }

  &__name {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 15px;
    font-weight: 600;
    line-height: 20px;

    i { color: var(--sw-accent); font-size: 13px; }
  }

  &__hint {
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 16px;
  }
}

/* Варианты акцента: ряд кнопок одной высоты, выбранная — в своём цвете. */
.pz-accents {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;

  &__label {
    margin-right: 8px;
    color: var(--l-text-dim);
    font-size: 12px;
    font-weight: 600;
    line-height: 16px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
}

.pz-accent {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  height: 32px;
  padding: 0 12px;
  border: none;
  border-radius: var(--l-radius-sm);
  background: var(--l-field-bg);
  box-shadow: inset 0 0 0 1px var(--l-modal-border);
  color: var(--l-text-dim);
  font: inherit;
  font-size: 13px;
  line-height: 16px;
  cursor: pointer;
  transition: box-shadow var(--l-fast), color var(--l-fast), background var(--l-fast);

  &:hover {
    color: var(--l-text);
    box-shadow: inset 0 0 0 1px var(--l-border-strong);
  }

  &.active {
    color: var(--l-text);
    background: color-mix(in srgb, var(--sw-accent) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sw-accent) 60%, transparent);
  }

  &__dot {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--sw-accent);
  }
}
</style>
