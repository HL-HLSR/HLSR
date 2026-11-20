<template>
  <div v-if="visible" class="style-picker">
    <!-- Сверху — пояснение и строка подписки (если подписка была). -->
    <div v-if="mode === 'grid' || (mode === 'none' && subscriberText)" class="style-picker__head">
      <p v-if="mode === 'grid'" class="style-picker__hint">{{ $t("#L_RUNNER_STYLE_HINT") }}</p>
      <span v-if="subscriberText" class="style-picker__sub" :class="{ 'is-active': subscriber.active }">
        <i class="fas fa-star" aria-hidden="true"></i>
        {{ subscriberText }}
      </span>
    </div>

    <!-- Сетка: «Без подсветки» и заготовки. Предпросмотр — свой ник и
         аватар в CSS заготовки, теми же NickName и AvatarFrame, которыми
         профиль рисуется у других игроков. Пока выбор сохраняется, карточки
         не выключаются (disabled сбросил бы фокус с клавиатуры) — второй
         выбор отсекает choose(). -->
    <div
      v-if="mode === 'grid'"
      class="style-picker__grid"
      role="radiogroup"
      :aria-label="$t('#L_RUNNER_STYLE_GROUP')"
      :aria-busy="pending !== null ? 'true' : 'false'"
    >
      <button
        v-for="option in options"
        :key="option.id"
        type="button"
        role="radio"
        class="style-picker__option"
        :class="{ 'is-selected': option.id === selected, 'is-pending': option.id === pending }"
        :aria-checked="option.id === selected ? 'true' : 'false'"
        :aria-disabled="pending !== null ? 'true' : null"
        @click="choose(option.id)"
      >
        <span class="style-picker__preview">
          <AvatarFrame :frame="option.frame" :width="3">
            <span class="style-picker__avatar">
              <img v-if="avatar" :src="avatar" alt="" />
              <i v-else class="fab fa-steam" aria-hidden="true"></i>
            </span>
          </AvatarFrame>
          <NickName :nick="option.nick" :subscriber="subscriberBadge" class="style-picker__nick">{{ name }}</NickName>
        </span>
        <span class="style-picker__title">{{ option.title }}</span>
        <span class="style-picker__mark" aria-hidden="true">
          <i v-if="option.id === pending" class="fas fa-spinner fa-spin"></i>
          <i v-else-if="option.id === selected" class="fas fa-check"></i>
        </span>
      </button>
    </div>

    <!-- Без права: пояснение и окно «Спонсорство». Заготовку, выбранную,
         пока право было, можно снять. -->
    <div v-else-if="mode === 'locked'" class="style-picker__locked">
      <span class="style-picker__heart" aria-hidden="true"><i class="fas fa-heart"></i></span>
      <div class="style-picker__locked-text">
        <span class="style-picker__locked-title">{{ $t("#L_RUNNER_STYLE_PERK_REQUIRED") }}</span>
        <span class="style-picker__locked-hint">{{ $t("#L_RUNNER_STYLE_LOCKED_HINT") }}</span>
        <span v-if="subscriberText" class="style-picker__sub is-inline">
          <i class="fas fa-star" aria-hidden="true"></i>
          {{ subscriberText }}
        </span>
      </div>
      <div class="style-picker__actions">
        <button v-if="current" type="button" class="style-picker__btn is-ghost" :disabled="pending !== null" @click="choose(0)">
          {{ $t("#L_RUNNER_STYLE_REMOVE") }}
        </button>
        <button type="button" class="style-picker__btn is-support" @click="support">
          <i class="fas fa-heart" aria-hidden="true"></i>
          {{ $t("#L_RUNNER_STYLE_SUPPORT") }}
        </button>
      </div>
    </div>

    <p v-if="message" class="style-picker__message" :class="{ 'is-error': tone === 'error' }" :role="tone === 'error' ? 'alert' : 'status'">
      <i :class="tone === 'error' ? 'fas fa-exclamation-circle' : 'fas fa-check'" aria-hidden="true"></i>
      {{ $t(message) }}
    </p>
  </div>
</template>

<script>
import NickName from "./NickName";
import AvatarFrame from "./AvatarFrame";

const { ipcRenderer } = window.hlsrElectron;

// Отказы сервера, у которых есть свой текст; остальные — общий.
const ERRORS = Object.freeze({
  RUNNER_PERK_REQUIRED: "#L_RUNNER_STYLE_PERK_REQUIRED",
  RUNNER_PRESET_UNKNOWN: "#L_RUNNER_STYLE_UNKNOWN",
  RUNNER_NETWORK: "#L_RUNNER_NETWORK",
});

// ДД.ММ.ГГГГ по часам игрока, как «В HLSR с …» в профиле (ProfileModal):
// сервер присылает момент конца периода в UTC, а день — тот, что у игрока.
function formatDate(iso) {
  const date = new Date(iso);
  if (!iso || isNaN(date.getTime())) return "";
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
}

// Выбор заготовки «подсветка ника + рамка» в настройках профиля. Право
// (perks.styles) и выбранную заготовку (current) присылает сервер в своём
// профиле, список заготовок (presets) запрашивает RunnerSettings. Выбор
// уходит в runner-customize { stylePreset } — 0 снимает заготовку; свой
// вызов, а не RunnerSettings.request(): тот теряет код отказа.
export default {
  name: "StylePresetPicker",
  components: { NickName, AvatarFrame },
  emits: ["saved", "open-sponsors", "stale"],
  props: {
    perks: { type: Object, default: () => ({ styles: false, subscriber: null }) },
    // Подписка активна сейчас (subscriber профиля): ник в предпросмотре — со
    // значком, как его видят другие.
    subscriberBadge: { type: Boolean, default: false },
    current: { type: Number, default: null },
    presets: { type: Array, default: () => [] },
    account: { type: Object, default: () => ({}) },
  },
  data() {
    return {
      // Номер заготовки, которая сохраняется сейчас; null — ничего.
      pending: null,
      message: null,
      tone: "ok",
    };
  },
  computed: {
    // grid — сетка выбора; locked — права нет; none — заготовок нет
    // (сервер на PHP или админка их ещё не завела).
    mode() {
      if (!this.presets.length) return "none";
      return this.perks && this.perks.styles === true ? "grid" : "locked";
    },
    // Нечего показать (заготовок нет, подписки не было) — блока нет совсем:
    // иначе между заголовком группы и строкой Telegram оставался бы пустой
    // отступ с разделителем.
    visible() {
      return this.mode !== "none" || !!this.subscriberText;
    },
    options() {
      return [{ id: 0, title: this.$t("#L_RUNNER_STYLE_NONE"), nick: null, frame: null }].concat(this.presets);
    },
    // Отмеченный вариант: 0 — без подсветки; null — выбрана заготовка,
    // которой в списке нет (её скрыли или удалили).
    selected() {
      if (!this.current) return 0;
      return this.presets.some((item) => item.id === this.current) ? this.current : null;
    },
    name() {
      return this.account.personaName || this.$t("#L_SB_STEAM_ACCOUNT");
    },
    avatar() {
      return this.account.avatarDataUri || null;
    },
    subscriber() {
      const value = this.perks && this.perks.subscriber;
      return value && typeof value === "object" ? value : null;
    },
    subscriberText() {
      const subscriber = this.subscriber;
      if (!subscriber) return null;
      if (!subscriber.active) return this.$t("#L_RUNNER_STYLE_SUB_ENDED");
      const until = formatDate(subscriber.until);
      return until ? this.$t("#L_RUNNER_STYLE_SUB_UNTIL", until) : this.$t("#L_RUNNER_STYLE_SUB_ACTIVE");
    },
  },
  methods: {
    async choose(id) {
      if (this.pending !== null || id === this.selected) return;
      if (id !== 0 && (this.mode !== "grid" || !this.presets.some((item) => item.id === id))) return;
      this.pending = id;
      this.message = null;
      try {
        const result = await ipcRenderer.invoke("runner-customize", { stylePreset: id });
        if (result && result.status !== "error" && result.snapshot) {
          this.$emit("saved", result.snapshot);
          this.tone = "ok";
          this.message = "#L_RUNNER_STYLE_SAVED";
          return;
        }
        const code = result && result.code;
        this.tone = "error";
        this.message = ERRORS[code] || "#L_RUNNER_STYLE_ERROR";
        // Заготовку скрыли или удалили: список перечитывается мимо кеша.
        if (code === "RUNNER_PRESET_UNKNOWN") this.$emit("stale");
      } catch (error) {
        this.tone = "error";
        this.message = "#L_RUNNER_NETWORK";
      } finally {
        this.pending = null;
      }
    },
    support() {
      this.$emit("open-sponsors");
    },
  },
};
</script>

<style lang="scss" scoped>
/* Оформление — как у строк и кнопок RunnerSettings; розовый — цвет окна
   «Спонсорство» и сердечка в шапке (GitHub Sponsors). */
.style-picker {
  --sponsor: #db61a2;
  padding: 12px 0 16px;
  box-shadow: inset 0 -1px 0 rgba(255, 255, 255, 0.045);

  &__head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px 16px;
    margin-bottom: 12px;
  }

  &__hint {
    margin: 0;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 20px;
  }

  &__sub {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text-dim);
    font-size: 12px;
    font-weight: 500;
    line-height: 20px;
    white-space: nowrap;

    i {
      font-size: 10px;
    }

    &.is-active {
      background: color-mix(in srgb, var(--sponsor) 12%, transparent);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 35%, transparent);
      color: var(--sponsor);
    }

    &.is-inline {
      align-self: flex-start;
      margin-top: 4px;
    }
  }

  &__grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(208px, 1fr));
    gap: 8px;
  }

  &__option {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
    min-width: 0;
    padding: 12px 14px;
    border: 0;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background var(--l-fast), box-shadow var(--l-fast), transform var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.06);
    }

    &:active:not([aria-disabled="true"]) {
      transform: scale(0.98);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: 2px;
    }

    &[aria-disabled="true"] {
      cursor: progress;
    }

    &.is-selected {
      background: color-mix(in srgb, var(--accent) 8%, transparent);
      box-shadow: inset 0 0 0 2px var(--accent);
    }
  }

  &__preview {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    min-width: 0;
    padding-right: 20px;
    box-sizing: border-box;
  }

  &__avatar {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    overflow: hidden;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.06);
    color: var(--l-text-dim);
    font-size: 16px;

    img {
      width: 100%;
      height: 100%;
      object-fit: cover;
    }
  }

  &__nick {
    /* Свечение значка подписчика и подсветки выходит за буквы, а overflow
       ради многоточия срезал бы его: поля под свечение съедаются
       отрицательным отступом, раскладка не меняется. */
    min-width: 0;
    margin: -4px -6px;
    padding: 4px 6px;
    overflow: hidden;
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__title {
    max-width: 100%;
    overflow: hidden;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__mark {
    position: absolute;
    top: 10px;
    right: 10px;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    font-size: 9px;
  }

  &__option.is-selected &__mark {
    background: var(--accent);
    color: var(--accent-contrast-text);
  }

  &__option.is-pending &__mark {
    color: var(--l-text-dim);
    font-size: 12px;
  }

  &__locked {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 16px;
    border-radius: var(--l-radius-sm);
    background: radial-gradient(120% 160% at 0% 0%, color-mix(in srgb, var(--sponsor) 10%, transparent) 0%, transparent 70%), rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 25%, var(--l-border));
  }

  &__heart {
    display: grid;
    flex: none;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--sponsor) 16%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--sponsor) 40%, transparent);
    color: var(--sponsor);
    font-size: 15px;
  }

  &__locked-text {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  &__locked-title {
    color: var(--l-text);
    font-size: 14px;
    font-weight: 500;
    line-height: 24px;
  }

  &__locked-hint {
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 20px;
  }

  &__actions {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
  }

  &__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 40px;
    padding: 8px 16px;
    border: none;
    border-radius: var(--l-radius-sm);
    font-family: inherit;
    font-size: 13px;
    font-weight: 600;
    line-height: 24px;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--l-fast), filter var(--l-fast), transform var(--l-fast);

    i {
      font-size: 12px;
    }

    &:active {
      transform: scale(0.97);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: 2px;
    }

    &:disabled {
      opacity: 0.45;
      cursor: default;
      transform: none;
    }

    &.is-support {
      background: var(--sponsor);
      box-shadow: 0 6px 22px color-mix(in srgb, var(--sponsor) 32%, transparent);
      color: #fff;

      &:hover {
        filter: brightness(1.08);
      }
    }

    &.is-ghost {
      background: transparent;
      color: var(--l-text-dim);

      &:hover:not(:disabled) {
        background: rgba(255, 255, 255, 0.06);
        color: var(--l-text);
      }
    }
  }

  &__message {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0 0;
    color: #6fd38a;
    font-size: 12px;
    line-height: 20px;

    &.is-error {
      color: #ff6b78;
    }
  }
}

@media (max-width: 760px) {
  .style-picker__locked {
    flex-direction: column;
    align-items: stretch;
  }
}

@media (prefers-reduced-motion: reduce) {
  .style-picker__option,
  .style-picker__btn {
    transition: none;
  }
}
</style>
