<template>
  <!-- Проверка конфига HL1 по таблице speedrun.com (только HL1): итог одной строкой и
       список групп «команда + правило» с числом мест. Раскрытая группа показывает места;
       нажатие на место открывает файл в редакторе cfg. Ничего не блокирует. -->
  <section class="gx-pane hc" :aria-label="$t('#L_HC_TITLE')">
    <header class="gx-pane__head">
      <h3 class="gx-pane__title">{{ $t("#L_HC_TITLE") }}</h3>
      <button
        type="button"
        class="gx-btn gx-btn--icon"
        :disabled="loading"
        :title="$t('#L_HC_REFRESH')"
        :aria-label="$t('#L_HC_REFRESH')"
        @click="$emit('refresh')"
      >
        <i class="fas fa-sync-alt" aria-hidden="true"></i>
      </button>
      <!-- Категория проверки: родные radio, стрелки и Tab работают без своего кода. -->
      <span class="hc__cat-label" aria-hidden="true">{{ $t("#L_HC_CATEGORY_LABEL") }}</span>
      <div class="hc__cat" role="radiogroup" :aria-label="$t('#L_HC_CATEGORY')">
        <label v-for="item in CATEGORIES" :key="item.id" class="hc__cat-item" :class="{ 'is-on': category === item.id }">
          <input
            type="radio"
            class="hc__cat-input"
            name="hc-category"
            :value="item.id"
            :checked="category === item.id"
            @change="$emit('category', item.id)"
          />{{ item.title }}
        </label>
      </div>
      <p class="gx-pane__note">{{ $t("#L_HC_NOTE") }}</p>
    </header>

    <div class="hc__body">
      <p v-if="!ready" class="hc__state" :class="{ 'is-failed': !loading }" :role="loading ? 'status' : 'alert'">
        <i v-if="!loading" class="fas fa-exclamation-triangle" aria-hidden="true"></i>{{ $t(loading ? "#L_HC_LOADING" : "#L_HC_FAILED") }}
      </p>
      <template v-else>
        <p v-if="failed" class="hc__state is-failed" role="alert">
          <i class="fas fa-exclamation-triangle" aria-hidden="true"></i>{{ $t("#L_HC_STALE") }}
        </p>
        <p class="hc__summary" role="status">
          <template v-if="!groups.length">
            <span class="hc__chip is-ok"><i class="fas fa-check-circle" aria-hidden="true"></i>{{ $t("#L_HC_CLEAN") }}</span>
          </template>
          <template v-else>
            <span v-if="stats.errors" class="hc__chip is-error"><i class="fas fa-times-circle" aria-hidden="true"></i>{{ $t("#L_HC_COUNT_ERRORS", String(stats.errors)) }}</span>
            <span v-if="stats.warnings" class="hc__chip is-warning"><i class="fas fa-exclamation-triangle" aria-hidden="true"></i>{{ $t("#L_HC_COUNT_WARNINGS", String(stats.warnings)) }}</span>
            <span v-if="stats.infos" class="hc__chip is-info"><i class="fas fa-info-circle" aria-hidden="true"></i>{{ $t("#L_HC_COUNT_INFOS", String(stats.infos)) }}</span>
          </template>
          <span class="hc__scope">{{ $t("#L_HC_FILES", String(stats.files), String(stats.commands)) }}</span>
        </p>

        <ul v-if="groups.length" class="hc__groups">
          <li v-for="group in groups" :key="group.key" class="hc__group" :class="'is-' + group.level">
            <button
              type="button"
              class="hc__row"
              :aria-expanded="isOpen(group) ? 'true' : 'false'"
              @click="toggle(group)"
            >
              <span class="hc__level"><i :class="'fas ' + ICONS[group.level]" aria-hidden="true"></i>{{ $t(LEVEL_KEYS[group.level]) }}</span>
              <code class="hc__name">{{ group.name }}</code>
              <span class="hc__reason">{{ reasonOf(group) }}</span>
              <span class="hc__count" :aria-label="$t('#L_HC_PLACES', String(group.places.length))">&times;&nbsp;{{ group.places.length }}</span>
              <i class="fas hc__chev" :class="isOpen(group) ? 'fa-chevron-up' : 'fa-chevron-down'" aria-hidden="true"></i>
            </button>
            <ul v-if="isOpen(group)" class="hc__places">
              <li v-for="(place, index) in group.places" :key="index">
                <button
                  type="button"
                  class="hc__place"
                  :disabled="!place.path"
                  :title="$t('#L_HC_PLACE_OPEN', place.file, String(place.line))"
                  :aria-label="$t('#L_HC_PLACE_OPEN', place.file, String(place.line)) + ': ' + place.command"
                  @click="$emit('open-place', place)"
                >
                  <code class="hc__where">{{ place.file }}<template v-if="place.line">:{{ place.line }}</template></code>
                  <code class="hc__cmd">{{ place.command }}</code>
                  <span class="hc__ctx">{{ contextOf(place) }}</span>
                </button>
              </li>
            </ul>
          </li>
        </ul>
      </template>
    </div>
  </section>
</template>

<script>
const { groupFindings, ruleDetail, RULE_KEYS } = require("../../../shared/hl1-config-groups");

const ICONS = { error: "fa-times-circle", warning: "fa-exclamation-triangle", info: "fa-info-circle" };
const LEVEL_KEYS = { error: "#L_HC_LEVEL_ERROR", warning: "#L_HC_LEVEL_WARNING", info: "#L_HC_LEVEL_INFO" };

export default {
  name: "GameCfgCheck",
  emits: ["open-place", "refresh", "category"],
  props: {
    // "scriptless" или "scripted": по какой категории считать находки.
    category: { type: String, default: "scriptless" },
    // Ответ player-config-check со status "ok" или null, пока он не получен.
    check: { type: Object, default: null },
    // Последняя перепроверка не удалась: показан прежний итог (или его нет вовсе).
    failed: { type: Boolean, default: false },
    loading: { type: Boolean, default: false },
  },
  data() {
    return { open: {}, ICONS, LEVEL_KEYS, CATEGORIES: [{ id: "scriptless", title: "Scriptless" }, { id: "scripted", title: "Scripted" }] };
  },
  computed: {
    ready() {
      return !!this.check && this.check.status === "ok";
    },
    stats() {
      return this.ready ? this.check.stats : { files: 0, commands: 0, errors: 0, warnings: 0, infos: 0 };
    },
    groups() {
      return this.ready ? groupFindings(this.check.findings) : [];
    },
  },
  methods: {
    isOpen(group) {
      return !!this.open[group.key];
    },
    toggle(group) {
      this.open = Object.assign({}, this.open, { [group.key]: !this.open[group.key] });
    },
    reasonOf(group) {
      const key = RULE_KEYS[group.rule];
      return key ? this.$t(key, ruleDetail(group)) : group.note;
    },
    contextOf(place) {
      return place.trail && place.trail.length ? this.trailText(place.trail) : this.$t("#L_HC_TOP_LEVEL");
    },
    // Путь до команды: alias и bind, из которых она запущена; exec — файл, зовущийся из них.
    trailText(trail) {
      return trail.map((step) => {
        if (step.kind === "exec") return this.$t("#L_HC_CTX_EXEC", this.trailText(step.via));
        return this.$t(step.kind === "bind" ? "#L_HC_CTX_BIND" : "#L_HC_CTX_ALIAS", step.name);
      }).join(" › ");
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

/* Цвета уровня — свои переменные окна; уровень всегда ещё и словом с значком. */
.hc {
  --hc-error: var(--l-level-error);
  --hc-warning: var(--l-level-warning);
  --hc-info: var(--l-text-dim);
  --hc-ok: var(--l-level-ok);
  height: 100%;
}

.hc__cat-label {
  flex: none;
  color: var(--l-text-dim);
  font-size: 12px;
}

.hc__cat {
  display: inline-flex;
  flex: none;
  padding: 2px;
  border-radius: 999px;
  background: var(--l-chip-bg);
}

.hc__cat-item {
  position: relative;
  padding: 3px 12px;
  border-radius: 999px;
  color: var(--l-text-dim);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;

  &.is-on { background: var(--accent-soft); color: var(--l-text); }
  &:focus-within { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
}

/* Настоящий radio остаётся в дереве доступности, только не виден. */
.hc__cat-input {
  position: absolute;
  inset: 0;
  margin: 0;
  opacity: 0;
  cursor: pointer;
}

.hc__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  @include gx-scrollbar;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 14px;
}

.hc__state {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  color: var(--l-text-dim);
  font-size: 13px;

  &.is-failed { color: var(--hc-warning); }
}

.hc__summary {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px 8px;
  margin: 0;
}

.hc__chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.06);
  font-size: 12.5px;
  font-weight: 600;
  line-height: 20px;

  &.is-error { color: var(--hc-error); }
  &.is-warning { color: var(--hc-warning); }
  &.is-info { color: var(--hc-info); }
  &.is-ok { color: var(--hc-ok); }
}

.hc__scope {
  color: var(--l-text-faint);
  font-size: 12px;
}

.hc__groups,
.hc__places {
  margin: 0;
  padding: 0;
  list-style: none;
}

.hc__group {
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.03);
  box-shadow: inset 3px 0 0 var(--hc-info);

  &.is-error { box-shadow: inset 3px 0 0 var(--hc-error); }
  &.is-warning { box-shadow: inset 3px 0 0 var(--hc-warning); }
  & + & { margin-top: 4px; }
}

.hc__row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 10px;
  width: 100%;
  min-height: 40px;
  padding: 6px 12px 6px 14px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--l-text);
  font: inherit;
  font-size: 13px;
  text-align: left;
  cursor: pointer;
  transition: background var(--l-fast);

  &:hover { background: rgba(255, 255, 255, 0.05); }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
}

.hc__level {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 6px;
  min-width: 128px;
  font-size: 12px;
  font-weight: 600;

  .is-error > .hc__row > & { color: var(--hc-error); }
  .is-warning > .hc__row > & { color: var(--hc-warning); }
  .is-info > .hc__row > & { color: var(--hc-info); }
}

.hc__name {
  flex: none;
  font-family: Consolas, monospace;
  font-size: 12.5px;
}

.hc__reason {
  flex: 1 1 200px;
  min-width: 0;
  color: var(--l-text-dim);
  overflow-wrap: anywhere;
}

.hc__count {
  flex: none;
  color: var(--l-text-dim);
  font-variant-numeric: tabular-nums;
}

.hc__chev {
  flex: none;
  color: var(--l-text-faint);
  font-size: 11px;
}

.hc__places {
  padding: 0 8px 8px 14px;
}

.hc__place {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 2px 10px;
  width: 100%;
  padding: 4px 8px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--l-text-dim);
  font: inherit;
  font-size: 12px;
  text-align: left;
  cursor: pointer;

  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.07); color: var(--l-text); }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
  &:disabled { cursor: default; opacity: 0.6; }
}

.hc__where {
  flex: none;
  color: var(--accent);
  font-family: Consolas, monospace;
}

.hc__cmd {
  min-width: 0;
  color: var(--l-text);
  font-family: Consolas, monospace;
  overflow-wrap: anywhere;
}

.hc__ctx {
  color: var(--l-text-faint);
  overflow-wrap: anywhere;
}

.no-anim .hc__row { transition: none; }

@media (prefers-reduced-motion: reduce) {
  .hc__row { transition: none; }
}
</style>
