<template>
  <!-- Место забега в таблице speedrun.com. Первые три места отмечены
       цветной лямбдой, как трофеи игр Half-Life на самом speedrun.com. -->
  <span
    v-if="place"
    class="run-place"
    :class="['run-place--' + size, medal ? 'run-place--' + medal : '']"
    :title="label"
  >
    <svg v-if="medal" class="run-place__lambda" viewBox="0 0 177 186" aria-hidden="true">
      <path
        d="M47.2 12.9v25.9h21.4l8.7 27.1-65.1 101h32l46.5-68.6 30.5 77 48-15.3-7.6-23.6-22.1 6.9-51.8-130.4z"
      />
    </svg>
    <span class="run-place__text">{{ label }}</span>
  </span>
</template>

<script>
const MEDALS = { 1: "gold", 2: "silver", 3: "bronze" };

export default {
  name: "RunPlace",
  props: {
    place: { type: Number, default: null },
    size: { type: String, default: "sm" },
  },
  computed: {
    medal() {
      return MEDALS[this.place] || "";
    },
    label() {
      if (this.$t("#L_RUNNER_LOCALE") !== "en") return this.$t("#L_RUN_PLACE", String(this.place));
      const tens = this.place % 100;
      const suffix = tens >= 11 && tens <= 13 ? "th" : ["th", "st", "nd", "rd"][this.place % 10] || "th";
      return this.$t("#L_RUN_PLACE", this.place + suffix);
    },
  },
};
</script>

<style lang="scss" scoped>
.run-place {
  --medal: var(--l-text-dim);
  --medal-glow: transparent;

  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex: none;
  color: var(--l-text-dim);
  font-size: 12px;
  font-weight: 600;
  line-height: 16px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;

  &--gold {
    --medal: #f5c542;
    --medal-glow: rgba(245, 197, 66, 0.45);
  }

  &--silver {
    --medal: #cfd8e3;
    --medal-glow: rgba(207, 216, 227, 0.35);
  }

  &--bronze {
    --medal: #d98c4a;
    --medal-glow: rgba(217, 140, 74, 0.4);
  }

  &--gold,
  &--silver,
  &--bronze {
    color: var(--medal);
  }

  &__lambda {
    width: 14px;
    height: 14px;
    fill: currentColor;
    filter: drop-shadow(0 0 4px var(--medal-glow));
  }

  &--lg {
    gap: 8px;
    font-size: 13px;
    line-height: 20px;
  }

  &--lg &__lambda {
    width: 20px;
    height: 20px;
  }
}
</style>
