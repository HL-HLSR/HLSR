<template>
  <div class="gx gx--wide gc gc--top">
    <nav class="gx-nav" role="tablist" aria-orientation="horizontal" :aria-label="$t('#L_GS_X_CATEGORIES')" @keydown="onNavKey">
      <button
        v-for="section in navSections"
        :key="section.id"
        ref="tabs"
        type="button"
        role="tab"
        class="gx-nav__item"
        :class="{ active: activeId === section.id }"
        :id="'gc-tab-' + section.id"
        :aria-selected="activeId === section.id ? 'true' : 'false'"
        :tabindex="activeId === section.id ? 0 : -1"
        aria-controls="gc-panel"
        @click="selectSection(section.id)"
      >
        <span class="gx-nav__icon">
          <i :class="'fas ' + section.icon" aria-hidden="true"></i>
          <span v-if="sectionEdits(section)" class="gx-nav__dot" aria-hidden="true"></span>
        </span>
        <span class="gx-nav__label">{{ text(section.title) }}</span>
        <!-- Итог проверки по правилам: значок и число, не только цвет. -->
        <span v-if="section.id === CHECK_ID && checkBadge" class="gc-badge" :class="'is-' + checkBadge.level">
          <i :class="'fas ' + checkBadge.icon" aria-hidden="true"></i>
          <template v-if="checkBadge.count">{{ checkBadge.count }}</template>
        </span>
        <span v-if="section.id === CHECK_ID && checkBadge" class="gx-sr">{{ checkBadge.text }}</span>
        <span v-if="sectionEdits(section)" class="gx-sr">{{ $t('#L_GS_CFG_EDITED', sectionEdits(section)) }}</span>
      </button>
    </nav>

    <!-- Редактор cfg-файлов — первый раздел; его содержимое передаёт окно
         настроек (слот files), потому что ему нужна выбранная сборка. -->
    <div v-if="activeSection && activeSection.id === FILES_ID" id="gc-panel" class="gc-files" role="tabpanel" :aria-labelledby="'gc-tab-' + FILES_ID">
      <slot name="files"></slot>
    </div>

    <div v-else-if="activeSection && activeSection.id === CHECK_ID" id="gc-panel" class="gc-files" role="tabpanel" :aria-labelledby="'gc-tab-' + CHECK_ID">
      <slot name="check"></slot>
    </div>

    <!-- «Игра»: все разделы каталога одной прокруткой (Управление, Изображение, Звук, Игра),
         сверху — якоря к ним. -->
    <section v-else-if="activeSection" id="gc-panel" class="gx-pane" role="tabpanel" :aria-labelledby="'gc-tab-' + activeSection.id">
      <header class="gx-pane__head">
        <nav v-if="activeSection.sections.length > 1" class="gc-jump" :aria-label="$t('#L_GS_CFG_JUMP')">
          <button v-for="section in activeSection.sections" :key="section.id" type="button" class="gc-jump__item" @click="jumpTo(section.id)">
            <span class="gx-nav__icon">
              <i :class="'fas ' + section.icon" aria-hidden="true"></i>
              <span v-if="sectionEdits(section)" class="gx-nav__dot" aria-hidden="true"></span>
            </span>
            <span>{{ text(section.title) }}</span>
            <span v-if="sectionEdits(section)" class="gx-sr">{{ $t('#L_GS_CFG_EDITED', sectionEdits(section)) }}</span>
          </button>
        </nav>
        <p class="gx-pane__note" :title="$t('#L_GS_CFG_NOTE', playerFile || 'userconfig.cfg')">{{ $t('#L_GS_CFG_NOTE', playerFile || 'userconfig.cfg') }}</p>
      </header>

      <div ref="scroll" class="gc-scroll">
        <section v-for="section in activeSection.sections" :key="section.id" class="gc-sec" :aria-labelledby="'gc-sec-' + section.id">
          <header class="gc-sec__head">
            <h3 :id="'gc-sec-' + section.id" class="gc-sec__title" tabindex="-1">
              <i :class="'fas ' + section.icon" aria-hidden="true"></i>{{ text(section.title) }}
            </h3>
            <button
              type="button"
              class="gx-btn"
              :disabled="disabled || !sectionNonDefault(section)"
              :title="$t('#L_GS_CFG_RESET_HINT')"
              :aria-label="$t('#L_GS_CFG_RESET_HINT')"
              :aria-describedby="'gc-sec-' + section.id"
              @click="resetSection(section)"
            >
              <i class="fas fa-undo-alt" aria-hidden="true"></i>
            </button>
          </header>

          <div class="gx-fields">
            <div v-for="(column, columnIndex) in columns(section)" :key="columnIndex" class="gx-col">
              <fieldset
                v-for="field in column"
                :key="field.cvar"
                class="gx-row"
                :class="['gx-row--' + field.type, { 'is-changed': isEdited(field), 'is-off': !isAvailable(field) }]"
                :disabled="disabled || !isAvailable(field)"
              >
                <div class="gx-row__main">
                  <label class="gx-row__label" :for="inputId(field)">{{ text(field.label) }}</label>

                  <label v-if="field.type === 'toggle' || field.type === 'invert'" class="gx-switch">
                    <input :id="inputId(field)" type="checkbox" :checked="!!current(field)" :aria-describedby="hintId(field)" @change="set(field, $event.target.checked)" />
                    <span class="gx-switch__track" aria-hidden="true"><span class="gx-switch__thumb"></span></span>
                  </label>

                  <span v-else-if="isNumeric(field)" class="gx-num has-step" :class="{ 'has-unit': field.type === 'percent' }">
                    <input
                      :id="inputId(field)"
                      class="gx-input gx-input--num"
                      type="number"
                      :min="minOf(field)"
                      :max="maxOf(field)"
                      :step="stepOf(field)"
                      :inputmode="stepOf(field) < 1 ? 'decimal' : 'numeric'"
                      :value="current(field)"
                      :aria-describedby="hintId(field)"
                      autocomplete="off"
                      @change="setNumber(field, $event.target)"
                    />
                    <span v-if="field.type === 'percent'" class="gx-num__unit" aria-hidden="true">%</span>
                    <GxNumStep />
                  </span>

                  <UiDropdown
                    v-else-if="field.type === 'select'"
                    class="gx-dropdown"
                    :toggle-id="inputId(field)"
                    :described-by="hintId(field)"
                    :items="selectItems(field)"
                    :value="current(field)"
                    align="right"
                    @input="set(field, $event)"
                  >
                    <template #toggle="{ open }">
                      <span class="gx-input gx-input--select" :class="{ open: open }">
                        <span class="gx-input__value">{{ selectLabel(field) }}</span>
                        <i class="fas fa-chevron-down gx-input__chev" aria-hidden="true"></i>
                      </span>
                    </template>
                  </UiDropdown>
                </div>

                <p v-if="field.hint || ownFile(field) || isShadowed(field)" :id="hintId(field)" class="gx-row__hint">
                  {{ field.hint ? text(field.hint) : '' }}
                  <span v-if="isShadowed(field)" class="gc-source" role="note">{{ $t('#L_GS_CFG_SHADOWED') }}</span>
                  <span v-if="ownFile(field)" class="gc-source" :title="$t(isLocked(field) ? '#L_GS_CFG_SOURCE_LOCKED' : '#L_GS_CFG_SOURCE_HINT', ownFile(field))">{{ ownFile(field) }}</span>
                </p>

                <div v-if="isNumeric(field)" class="gx-slider" :class="{ 'has-scale': !!field.scale }">
                  <span v-if="field.scale" class="gx-slider__end" aria-hidden="true">{{ text(field.scale[0]) }}</span>
                  <span class="gx-slider__track" :class="{ 'is-default': isDefault(field) }" :style="sliderStyle(field)">
                    <span class="gx-slider__default" aria-hidden="true"></span>
                    <input
                      class="gx-range"
                      type="range"
                      :min="minOf(field)"
                      :max="maxOf(field)"
                      step="any"
                      :value="sliderValue(field)"
                      :aria-label="text(field.label)"
                      :aria-valuetext="current(field) + (field.type === 'percent' ? '%' : '')"
                      :title="$t('#L_GS_X_SLIDER_DEFAULT', defaultOf(field) + (field.type === 'percent' ? '%' : ''))"
                      @input="slide(field, $event.target)"
                      @change="endSlide(field, $event.target)"
                      @dblclick="set(field, defaultOf(field))"
                    />
                  </span>
                  <span v-if="field.scale" class="gx-slider__end" aria-hidden="true">{{ text(field.scale[1]) }}</span>
                </div>

                <p v-if="!isAvailable(field)" class="gx-row__needs">{{ $t('#L_GS_X_REQUIRES', text(requirement(field).label)) }}</p>
              </fieldset>
            </div>
          </div>
        </section>
      </div>
    </section>
  </div>
</template>

<script>
import orderedRefs from "@/utils/orderedRefs";
import UiDropdown from "./UiDropdown";
import GxNumStep from "./GxNumStep";
const gameConfig = require("../../../shared/game-config");
const { configTabs, FILES_ID, CHECK_ID, GAME_ID } = require("./config-tabs");

// Насколько близко к отметке значения по умолчанию, в пикселях, ползунок
// к ней прилипает (как в GhostingSettingsExtras).
const SLIDER_DETENT_PX = 6;
// Файлы, куда лаунчер пишет сам: их имя возле настройки ничего не говорит.
const PLAIN_FILES = new Set(["config.cfg"]);

// Понятные настройки игры вместо переменных cfg. values — строки из cfg
// (как их прочитал main), value — правки в единицах окна, ещё не записанные.
export default {
  name: "GameConfigEditor",
  components: { GxNumStep, UiDropdown },
  emits: ["input"],
  props: {
    gameId: { type: String, required: true },
    engine: { type: String, required: true },
    values: { type: Object, default: () => ({}) },
    sources: { type: Object, default: () => ({}) },
    shadowed: { type: Array, default: () => [] },
    // Имя файла игрока (userconfig.cfg / autoexec.cfg): чип с ним не показываем.
    playerFile: { type: String, default: "" },
    value: { type: Object, default: () => ({}) },
    disabled: { type: Boolean, default: false },
    // Итог проверки по правилам { errors, warnings } или null — тогда раздела «Проверка» нет.
    checkCounts: { type: Object, default: null },
    // Спрашивает, можно ли уйти из «Файлов cfg» с несохранённым текстом (true — можно).
    canLeave: { type: Function, default: null },
    // "side" — подразделы колонкой слева (окно), "top" — строкой вкладок над содержимым
    // (страница «Персонализация»: колонка слева там уже есть).
  },
  data() {
    return { activeId: null, sliding: {}, FILES_ID, CHECK_ID };
  },
  beforeUnmount() {
    if (this.slideFrame) cancelAnimationFrame(this.slideFrame);
  },
  computed: {
    catalog() {
      return gameConfig.catalogFor(this.gameId, this.engine);
    },
    // «Файлы cfg» (если окно передало редактор файлов), «Проверка» (слот check и итог, только HL1), «Игра».
    navSections() {
      return configTabs(this.catalog, { files: !!this.$slots.files, check: !!(this.$slots.check && this.checkCounts) });
    },
    // По умолчанию открыта «Игра».
    activeSection() {
      const tabs = this.navSections;
      return tabs.find((tab) => tab.id === this.activeId) || tabs.find((tab) => tab.id === GAME_ID) || tabs[0] || null;
    },
    checkBadge() {
      const counts = this.checkCounts;
      if (!counts) return null;
      if (counts.failed) return { level: "warning", icon: "fa-exclamation-triangle", count: 0, text: this.$t("#L_HC_FAILED") };
      if (counts.errors) return { level: "error", icon: "fa-times-circle", count: counts.errors, text: this.$t("#L_HC_COUNT_ERRORS", String(counts.errors)) };
      if (counts.warnings) return { level: "warning", icon: "fa-exclamation-triangle", count: counts.warnings, text: this.$t("#L_HC_COUNT_WARNINGS", String(counts.warnings)) };
      return { level: "ok", icon: "fa-check-circle", count: 0, text: this.$t("#L_HC_CLEAN") };
    },
    lang() {
      return Object.keys(this.$localization.locales)[this.$localization.lang] || "en";
    },
  },
  methods: {
    text(value) {
      return value ? value[this.lang] || value.en || "" : "";
    },
    inputId(field) {
      return "game-cfg-" + field.cvar;
    },
    hintId(field) {
      return field.hint || this.ownFile(field) || this.isShadowed(field) ? this.inputId(field) + "-hint" : null;
    },
    // Переменная задана в файле игрока несколько раз: верхние строки перекрыты нижней.
    isShadowed(field) {
      return this.shadowed.includes(field.cvar.toLowerCase());
    },
    isNumeric(field) {
      return field.type === "range" || field.type === "percent";
    },
    minOf(field) {
      return field.type === "percent" ? 0 : field.min;
    },
    maxOf(field) {
      return field.type === "percent" ? 100 : field.max;
    },
    stepOf(field) {
      return field.type === "percent" ? 1 : field.step || 1;
    },
    defaultOf(field) {
      return gameConfig.defaultValue(field);
    },
    // Значение, которое игра получит сейчас: из cfg или по умолчанию.
    saved(field) {
      const value = gameConfig.fromCfg(field, this.values[field.cvar]);
      return value === undefined ? this.defaultOf(field) : value;
    },
    isEdited(field) {
      return Object.prototype.hasOwnProperty.call(this.value, field.cvar);
    },
    current(field) {
      return this.isEdited(field) ? this.value[field.cvar] : this.saved(field);
    },
    isDefault(field) {
      return this.current(field) === this.defaultOf(field);
    },
    // Своё имя файла показывается, только если значение задано в cfg игрока
    // (autoexec.cfg и т. п.): правка уйдёт туда же.
    ownFile(field) {
      const file = this.sources[field.cvar];
      const lower = file ? file.toLowerCase() : "";
      return file && !PLAIN_FILES.has(lower) && lower !== this.playerFile.toLowerCase() ? file : "";
    },
    // Файл, который лаунчер не правит на месте (WON.cfg).
    isLocked(field) {
      const file = this.sources[field.cvar];
      return !!file && gameConfig.GENERATED_FILES.has(file.toLowerCase());
    },
    requirement(field) {
      return this.catalog.byCvar.get(field.requires.toLowerCase());
    },
    isAvailable(field) {
      return !field.requires || !!this.current(this.requirement(field));
    },
    sectionEdits(section) {
      return section.fields.filter((field) => this.isEdited(field)).length;
    },
    sectionNonDefault(section) {
      return section.fields.filter((field) => !this.isDefault(field)).length;
    },
    // Две независимые колонки: короткая настройка рядом с высокой не
    // оставляет дыры.
    columns(section) {
      const middle = Math.ceil(section.fields.length / 2);
      return [section.fields.slice(0, middle), section.fields.slice(middle)];
    },
    // Якорь: раздел к верху прокрутки, фокус на его заголовок (Tab идёт дальше по разделу).
    jumpTo(id) {
      const box = this.$refs.scroll;
      const title = box && box.querySelector("#gc-sec-" + id);
      if (!title) return;
      box.scrollTop = title.closest(".gc-sec__head").offsetTop;
      title.focus({ preventScroll: true });
    },
    selectItems(field) {
      return field.options.map((option) => ({ id: option.value, label: this.text(option.label) }));
    },
    selectLabel(field) {
      const found = field.options.find((option) => option.value === this.current(field));
      return found ? this.text(found.label) : String(this.current(field));
    },
    // Значение, равное записанному в cfg, — уже не правка.
    set(field, next) {
      const edits = Object.assign({}, this.value);
      const stored = gameConfig.fromCfg(field, this.values[field.cvar]);
      if (next === stored || (stored === undefined && next === this.defaultOf(field))) delete edits[field.cvar];
      else edits[field.cvar] = next;
      this.$emit("input", edits);
    },
    snap(field, raw) {
      const min = this.minOf(field), step = this.stepOf(field);
      const value = min + Math.round((Number(raw) - min) / step) * step;
      return Number(Math.max(min, Math.min(this.maxOf(field), value)).toFixed(6));
    },
    setNumber(field, input) {
      const raw = Number(input.value);
      if (input.value === "" || !Number.isFinite(raw)) {
        input.value = String(this.current(field));
        return;
      }
      const value = this.snap(field, raw);
      input.value = String(value);
      this.set(field, value);
    },
    sliderValue(field) {
      return Object.prototype.hasOwnProperty.call(this.sliding, field.cvar) ? this.sliding[field.cvar] : this.current(field);
    },
    sliderStyle(field) {
      const min = this.minOf(field), span = this.maxOf(field) - min;
      const at = (v) => Math.max(0, Math.min(100, ((Number(v) - min) / span) * 100));
      const value = at(this.sliderValue(field)), base = at(this.defaultOf(field));
      return { "--value": value + "%", "--default": base + "%", "--from": Math.min(value, base) + "%", "--to": Math.max(value, base) + "%" };
    },
    // Пока ползунок тянут, он идёт за курсором, а значение берёт ближайший
    // шаг не чаще раза за кадр.
    slide(field, input) {
      const raw = this.detent(field, input);
      this.sliding[field.cvar] = raw;
      this.pendingSlide = { field, raw };
      if (this.slideFrame) return;
      this.slideFrame = requestAnimationFrame(() => {
        this.slideFrame = 0;
        const next = this.pendingSlide;
        this.pendingSlide = null;
        if (next) this.set(next.field, this.snap(next.field, next.raw));
      });
    },
    endSlide(field, input) {
      if (this.slideFrame) {
        cancelAnimationFrame(this.slideFrame);
        this.slideFrame = 0;
        this.pendingSlide = null;
      }
      delete this.sliding[field.cvar];
      this.set(field, this.snap(field, this.detent(field, input)));
    },
    detent(field, input) {
      const raw = Number(input.value), width = input.offsetWidth;
      const base = this.defaultOf(field), span = this.maxOf(field) - this.minOf(field);
      if (!width) return raw;
      return (Math.abs(raw - base) / span) * width <= SLIDER_DETENT_PX ? base : raw;
    },
    resetSection(section) {
      const edits = Object.assign({}, this.value);
      for (const field of section.fields) {
        const base = this.defaultOf(field);
        const stored = gameConfig.fromCfg(field, this.values[field.cvar]);
        if (stored === base || (stored === undefined)) delete edits[field.cvar];
        else edits[field.cvar] = base;
      }
      this.$emit("input", edits);
    },
    // false — остались в прежнем разделе: в «Файлах cfg» есть несохранённый текст, и игрок ответил «Остаться».
    async selectSection(id) {
      const leaving = this.activeSection && this.activeSection.id === FILES_ID && id !== FILES_ID;
      if (leaving && this.canLeave && !(await this.canLeave())) return false;
      this.activeId = id;
      this.$nextTick(() => {
        if (this.$refs.scroll) this.$refs.scroll.scrollTop = 0;
      });
      return true;
    },
    async onNavKey(event) {
      const count = this.navSections.length;
      let index = this.navSections.findIndex((section) => section === this.activeSection);
      if (event.key === "ArrowDown" || event.key === "ArrowRight") index = (index + 1) % count;
      else if (event.key === "ArrowUp" || event.key === "ArrowLeft") index = (index - 1 + count) % count;
      else if (event.key === "Home") index = 0;
      else if (event.key === "End") index = count - 1;
      else return;
      event.preventDefault();
      const moved = await this.selectSection(this.navSections[index].id);
      if (!moved) index = this.navSections.findIndex((section) => section === this.activeSection);
      this.$nextTick(() => {
        const tab = orderedRefs(this.$refs.tabs)[index];
        if (tab) tab.focus();
      });
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

.gc {
  --gx-nav: 176px;
}

/* Подразделы строкой над содержимым: без своей подложки и рамки, как вкладки. */
.gc--top {
  grid-template: auto minmax(0, 1fr) / minmax(0, 1fr);

  .gx-nav {
    flex-direction: row;
    flex-wrap: wrap;
    gap: 4px;
    padding: 0;
    overflow: visible;
    border: 0;
    background: none;
  }
  .gx-nav__item { flex: none; white-space: nowrap; }
  .gx-nav__dot { box-shadow: none; }
}

/* Якоря к разделам «Игры»: строка простых кнопок, без подложки. */
.gc-jump {
  display: flex;
  flex-wrap: wrap;
  gap: 2px;
  margin-left: -8px;
}
.gc-jump__item {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  height: 28px;
  padding: 0 8px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--l-text-dim);
  font: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: background var(--l-fast), color var(--l-fast);

  &:hover { background: var(--l-chip-hover); color: var(--l-text); }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
  .gx-nav__dot { box-shadow: none; }
}

/* Разделы «Игры» одной прокруткой: заголовок как у групп настроек лаунчера,
   между разделами — тонкая линия. */
.gc-scroll {
  position: relative;
  flex: 1;
  min-height: 0;
  padding-bottom: 8px;
  overflow-y: auto;
  overscroll-behavior: contain;
  scroll-behavior: smooth;
  @include gx-scrollbar;
}
.no-anim .gc-scroll { scroll-behavior: auto; }
@media (prefers-reduced-motion: reduce) {
  .gc-scroll { scroll-behavior: auto; }
}
.gc-sec + .gc-sec::before {
  content: '';
  display: block;
  height: 1px;
  margin: 10px 14px 0;
  background: var(--l-border);
}
.gc-sec__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 14px 2px;
}
.gc-sec__title {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  color: var(--l-text-dim);
  font-size: 12px;
  font-weight: 600;
  line-height: 16px;
  letter-spacing: 0.08em;
  text-transform: uppercase;

  i { width: 14px; color: var(--l-text-faint); font-size: 12px; text-align: center; }
  &:focus { outline: none; }
}
.gc-sec .gx-fields { overflow: visible; }

/* Строка — карточка со скруглёнными углами: подсветка при наведении и
   изменённая настройка не упираются плоско в края колонки. Разделитель —
   ::after с отступами, чтобы рамка не загибалась вслед за углами. */
.gc .gx-fields {
  column-gap: 4px;
  padding: 6px 2px;
}
.gc .gx-row {
  padding-inline: 12px;
  border-bottom: 0;
  border-radius: 8px;

  &::after {
    content: '';
    position: absolute;
    right: 12px;
    bottom: 0;
    left: 12px;
    height: 1px;
    background: rgba(255, 255, 255, 0.055);
    pointer-events: none;
  }
  &:last-child::after,
  &.is-changed::after,
  &:hover::after,
  &:focus-within::after {
    background: transparent;
  }
  &:hover,
  &:focus-within {
    background: rgba(255, 255, 255, 0.04);
  }
  &::before {
    top: 12px;
    bottom: 12px;
    left: 4px;
    border-radius: 3px;
  }
  &.is-changed {
    background: color-mix(in srgb, var(--accent) 7%, transparent);
  }
  &.is-changed:hover,
  &.is-changed:focus-within {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
}

/* Свой cfg игрока, откуда взято значение. */
.gc-source {
  display: inline-block;
  margin-left: 4px;
  padding: 0 6px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.06);
  color: var(--l-text-dim);
  font-family: Consolas, monospace;
  font-size: 11px;
  line-height: 17px;
  vertical-align: 1px;
}

/* Итог проверки в пункте меню: значок и число. */
.gc-badge {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 4px;
  padding: 0 6px;
  border-radius: 9px;
  background: rgba(255, 255, 255, 0.08);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  font-weight: 600;
  line-height: 18px;

  i { font-size: 10px; }
  &.is-error { color: var(--l-level-error); }
  &.is-warning { color: var(--l-level-warning); }
  &.is-ok { color: var(--l-level-ok); }
}

.gc-files {
  min-width: 0;
  min-height: 0;
}

.gx-row:disabled .gx-row__label {
  cursor: default;
}

/* Full HD (окно 648–676 px): якоря и заметка одной строкой (заметка целиком — в подсказке),
   поля тоньше — под параметры остаётся 3–4 строки. */
@media (max-height: 700px) {
  .gx-pane__head { grid-template-columns: auto minmax(0, 1fr); padding-block: 6px; }
  .gx-pane__note { grid-column: auto; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .gc-sec__head { padding-top: 8px; }
}
</style>
