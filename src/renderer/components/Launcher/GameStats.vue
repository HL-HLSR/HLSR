<template>
  <div class="game-stats">
    <div class="game-stats__row" v-for="row in rows" :key="row.label">
      <span class="game-stats__icon">
        <i :class="row.icon"></i>
      </span>
      <span class="game-stats__divider"></span>
      <span class="game-stats__texts">
        <span class="game-stats__label">{{ row.label }}</span>
        <!-- Меняется только значение: оно перетекает в новое на месте. -->
        <span class="game-stats__value">
          <transition name="l-value">
            <span :key="row.value">{{ row.value }}</span>
          </transition>
        </span>
      </span>
    </div>
  </div>
</template>

<script>
export default {
  name: "GameStats",
  props: {
    game: {
      type: Object,
      required: true,
    },
    stats: {
      type: Object,
      default: null,
    },
    delivery: {
      type: Object,
      default: null,
    },
  },
  computed: {
    honest() {
      return this.delivery !== null || this.stats !== null;
    },
    rows() {
      return [
        {
          icon: "fas fa-play-circle",
          label: this.$t("#L_LAST_LAUNCH"),
          value: this.lastLaunchText,
        },
        {
          icon: "fas fa-clock",
          label: this.$t("#L_TOTAL_PLAYTIME"),
          value: this.playtimeText,
        },
        {
          icon: "fas fa-sync",
          label: this.$t("#L_BUILD_UPDATED"),
          value: this.buildUpdatedText,
        },
      ];
    },
    lastLaunchText() {
      if (this.honest) {
        if (!this.stats || !this.stats.available || !this.stats.lastLaunchAt)
          return this.$t("#L_NO_DATA");
        return this.relativeDate(this.stats.lastLaunchAt);
      }

      let days = this.game.demo.lastLaunchDays;
      if (days === null) return this.$t("#L_NEVER");
      if (days === 0) return this.$t("#L_TODAY");
      if (days === 1) return this.$t("#L_YESTERDAY");
      return this.$t("#L_DAYS_AGO", days);
    },
    playtimeText() {
      if (this.honest) {
        if (!this.stats || !this.stats.available) return this.$t("#L_NO_DATA");
        return this.durationText(this.stats.totalPlaySeconds || 0);
      }

      return this.$t("#L_HOURS", this.game.playtimeHours);
    },
    buildUpdatedText() {
      if (this.honest) {
        let at = this.delivery ? this.delivery.buildDate : null;
        if (!at) return this.$t("#L_NO_DATA");
        return this.absoluteDate(at);
      }

      return this.game.demo.buildUpdated;
    },
  },
  methods: {
    relativeDate(iso) {
      let date = new Date(iso);
      if (isNaN(date.getTime())) return this.$t("#L_NO_DATA");

      let startOfDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
      let days = Math.round((startOfDay(new Date()) - startOfDay(date)) / 86400000);

      if (days <= 0) return this.$t("#L_TODAY");
      if (days === 1) return this.$t("#L_YESTERDAY");
      if (days <= 6) return this.$t("#L_DAYS_AGO", days);
      return this.absoluteDate(iso);
    },
    absoluteDate(iso) {
      let date = new Date(iso);
      if (isNaN(date.getTime())) return this.$t("#L_NO_DATA");

      let pad = (n) => String(n).padStart(2, "0");
      return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
    },
    durationText(totalSeconds) {
      let minutes = Math.floor(totalSeconds / 60);
      let hours = Math.floor(minutes / 60);
      let rest = minutes % 60;

      if (hours > 0) return this.$t("#L_DUR_HM", String(hours), String(rest));
      return this.$t("#L_DUR_M", String(minutes));
    },
  },
};
</script>

<style lang="scss" scoped>
/* Строка статистики: область значка и столбик «подпись + значение» одной
   высоты (--l-stats-icon), линия между ними места не занимает. Так
   подпись, значение и значки всех трёх строк стоят на сетке. */
.game-stats {
  display: flex;
  flex-direction: column;
  gap: var(--l-stats-gap);

  &__row {
    display: flex;
    align-items: center;
    gap: 20px;
  }

  &__icon {
    width: var(--l-stats-icon);
    height: var(--l-stats-icon);
    min-width: var(--l-stats-icon);
    display: flex;
    align-items: center;
    // Значок прижат к разделителю: с обеих сторон линии один gap 20 px.
    // Ширина колонки сохраняется, поэтому текст остаётся на своём месте.
    justify-content: flex-end;
    color: #fff;
    font-size: 22px;

    /* У всех глифов одинаковая область 24 px рядом с разделителем. */
    i {
      width: 24px;
      line-height: 24px;
      text-align: center;
    }
  }

  &__divider {
    position: relative;
    width: 0;
    height: var(--l-stats-icon);

    &::after {
      content: "";
      position: absolute;
      inset: 0 auto 0 0;
      width: 1px;
      background: rgba(255, 255, 255, 0.28);
    }
  }

  &__texts {
    display: flex;
    flex-direction: column;
    gap: var(--l-stats-texts-gap);
  }

  /* Теней у текста статистики нет по той же причине, что и у заголовка:
     затемнение делает один общий слой, а вторая размытая копия букв
     поверх него читаемости не добавляет. */
  &__label {
    color: rgba(255, 255, 255, 0.62);
    font-size: 14px;
    font-weight: 500;
    line-height: 20px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
  }

  &__value {
    display: grid;
    color: var(--l-text);
    font-size: var(--l-stats-value);
    font-weight: 700;
    line-height: var(--l-stats-value-lh);
    font-variant-numeric: tabular-nums;

    > * { grid-area: 1 / 1; }
  }
}
</style>
