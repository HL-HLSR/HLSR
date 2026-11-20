<template>
  <div class="ls-launch" @mousedown.self="cancel">
    <div
      class="ls-launch__window"
      role="dialog"
      aria-modal="true"
      :aria-label="$t('#L_TOOL_LIVESPLIT_GAME_TITLE')"
      ref="window"
    >
      <span class="ls-launch__title">{{ $t("#L_TOOL_LIVESPLIT_GAME_TITLE") }}</span>
      <p class="ls-launch__text">{{ $t(isLinux ? "#L_TOOL_LIVESPLIT_GAME_TEXT" : "#L_TOOL_LIVESPLIT_GAME_TEXT_WIN") }}</p>

      <span class="ls-launch__label">{{ $t("#L_TOOL_LIVESPLIT_GAME") }}</span>
      <div class="ls-launch__choices" role="radiogroup" :aria-label="$t('#L_TOOL_LIVESPLIT_GAME')">
        <button
          v-for="game in games"
          :key="game.id"
          type="button"
          role="radio"
          class="ls-launch__choice"
          :class="{ active: game.id === gameId }"
          :aria-checked="game.id === gameId ? 'true' : 'false'"
          @click="pickGame(game.id)"
        >
          <i :class="game.id === gameId ? 'fas fa-dot-circle' : 'far fa-circle'" aria-hidden="true"></i>
          <span>{{ game.label }}</span>
        </button>
      </div>

      <template v-if="presets.length">
        <span class="ls-launch__label">{{ $t("#L_TOOL_SPLITS") }}</span>
        <div class="ls-launch__choices ls-launch__choices--grid" role="radiogroup" :aria-label="$t('#L_TOOL_SPLITS')">
          <button
            v-for="preset in presets"
            :key="preset.id"
            type="button"
            role="radio"
            class="ls-launch__choice"
            :class="{ active: preset.id === presetId }"
            :aria-checked="preset.id === presetId ? 'true' : 'false'"
            @click="presetId = preset.id"
            @dblclick="presetId = preset.id; accept()"
          >
            <i :class="preset.id === presetId ? 'fas fa-dot-circle' : 'far fa-circle'" aria-hidden="true"></i>
            <span class="ls-launch__preset">
              <span>{{ preset.category || $t(kindLabel(preset.kind)) }}</span>
              <small v-if="preset.category">{{ $t(kindLabel(preset.kind)) }}</small>
            </span>
          </button>
        </div>
      </template>
      <p v-else class="ls-launch__empty">{{ $t("#L_TOOL_SPLITS_NONE") }}</p>

      <div class="ls-launch__actions">
        <button type="button" class="ls-launch__btn" ref="cancelButton" @click="cancel">
          {{ $t("#L_CANCEL") }}
        </button>
        <button type="button" class="ls-launch__btn primary" :disabled="!gameId" @click="accept">
          {{ $t("#L_TOOL_LIVESPLIT_GAME_LAUNCH") }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";

// Окно запуска LiveSplit: для какой игры (на Linux — в её префиксе Proton,
// на Windows — для её оверлея) и с какими сплитами. Отвечает
// { game, gameId, preset } или false.
export default {
  name: "LiveSplitLaunchModal",
  emits: ["resolve"],
  props: {
    // [{ id, gameId, label, presets: [{ id, kind, category }] }]
    games: {
      type: Array,
      required: true,
    },
    defaultGame: {
      type: String,
      default: null,
    },
    // Сплиты, выбранные раньше: { gameId: id набора }.
    chosen: {
      type: Object,
      default: () => ({}),
    },
  },
  data() {
    const ids = this.games.map((game) => game.id);
    const gameId = ids.includes(this.defaultGame) ? this.defaultGame : ids[0] || null;
    return { gameId, presetId: null };
  },
  computed: {
    isLinux() {
      return window.hlsrElectron.platform === "linux";
    },
    game() {
      return this.games.find((game) => game.id === this.gameId) || null;
    },
    presets() {
      return (this.game && this.game.presets) || [];
    },
  },
  created() {
    this.presetId = this.rememberedPreset();
  },
  methods: {
    kindLabel(kind) {
      return kind === "maps" ? "#L_TOOL_SPLITS_MAPS" : "#L_TOOL_SPLITS_CHAPTERS";
    },
    rememberedPreset() {
      const saved = this.game ? this.chosen[this.game.gameId] : null;
      if (this.presets.some((preset) => preset.id === saved)) return saved;
      return this.presets.length ? this.presets[0].id : null;
    },
    pickGame(id) {
      if (id === this.gameId) return;
      this.gameId = id;
      this.presetId = this.rememberedPreset();
    },
    accept() {
      if (!this.game) return;
      this.$emit("resolve", { game: this.game.id, gameId: this.game.gameId, preset: this.presetId });
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
.ls-launch {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 70;

  &__window {
    width: min(520px, calc(100% - 80px));
    max-height: calc(100% - 60px);
    overflow-y: auto;
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

  &__label {
    margin-top: 18px;
    color: var(--l-text-dim);
    font-size: 11.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  &__choices {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 8px;

    &--grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
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
    transition: background var(--l-fast), border-color var(--l-fast), color var(--l-fast), transform var(--l-fast);

    i {
      flex: none;
      font-size: 13px;
    }

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      color: var(--l-text);
    }

    &:active {
      transform: scale(0.98);
    }

    &.active {
      border-color: var(--accent);
      color: var(--l-text);

      i {
        color: var(--accent);
      }
    }
  }

  &__preset {
    display: flex;
    flex-direction: column;
    min-width: 0;

    small {
      color: var(--l-text-dim);
      font-size: 11.5px;
      margin-top: 1px;
    }
  }

  &__empty {
    margin: 14px 0 0;
    color: var(--l-text-dim);
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
    }
  }
}
</style>
