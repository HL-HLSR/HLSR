<template>
  <!-- Оверлей LiveSplit в окне игры (src/shared/livesplit-overlay.js). Поля
       в тех же строках, что и остальные разделы; положение, размер и маска
       сразу видны в предпросмотре рядом, а картинку можно двигать мышью. -->
  <section id="gx-panel" class="gx-pane lso" role="tabpanel" aria-labelledby="gx-tab-livesplit">
    <header class="gx-pane__head">
      <h3 class="gx-pane__title">{{ $t('#L_LSO_TITLE') }}</h3>
      <button type="button" class="gx-btn gx-pane__reset" :disabled="!changes" :title="$t('#L_GS_X_RESET_HINT')" :aria-label="resetLabel" @click="reset">
        <i class="fas fa-undo-alt" aria-hidden="true"></i>
        <b v-if="changes" class="gx-btn__count" aria-hidden="true">{{ changes }}</b>
      </button>
      <p class="gx-pane__note">{{ $t('#L_LSO_NOTE') }}</p>
    </header>

    <div class="gx-fields">
      <!-- Что сейчас в предпросмотре: снимок настоящего окна или пример. -->
      <div class="lso-source" :class="['is-' + sourceTone, { 'is-flash': flash }]" role="status" @animationend="flash = false">
        <i class="fas" :class="sourceIcon" aria-hidden="true"></i>
        <span class="lso-source__text">
          <strong>{{ $t(sourceTitle) }}</strong>
          <span>{{ sourceHint }}</span>
        </span>
        <button type="button" class="gx-btn lso-source__refresh" :disabled="capturing" :aria-busy="capturing ? 'true' : 'false'" @click="refresh">
          <i class="fas fa-sync-alt" :class="{ 'fa-spin': capturing }" aria-hidden="true"></i>{{ $t('#L_LSO_REFRESH') }}
        </button>
      </div>

      <fieldset class="gx-row" :class="{ 'is-changed': overlay.enabled }">
        <div class="gx-row__main">
          <label class="gx-row__label" for="lso-enabled">{{ $t('#L_LSO_ENABLED') }}</label>
          <label class="gx-switch">
            <input id="lso-enabled" type="checkbox" :checked="overlay.enabled" aria-describedby="lso-enabled-hint" @change="set({ enabled: $event.target.checked })" />
            <span class="gx-switch__track" aria-hidden="true"><span class="gx-switch__thumb"></span></span>
          </label>
        </div>
        <p id="lso-enabled-hint" class="gx-row__hint">{{ $t('#L_LSO_ENABLED_HINT') }}</p>
      </fieldset>

      <fieldset class="gx-row" :class="{ 'is-changed': changed('x') || changed('y') }">
        <div class="gx-row__main">
          <span class="gx-row__label">{{ $t('#L_LSO_POSITION') }}</span>
          <!-- Углы и края кадра; середина — по центру. -->
          <div class="lso-anchors" role="group" :aria-label="$t('#L_LSO_POSITION')">
            <button
              v-for="anchor in anchors"
              :key="anchor.id"
              type="button"
              class="lso-anchors__cell"
              :class="{ active: overlay.x === anchor.x && overlay.y === anchor.y }"
              :style="{ gridColumn: anchor.x / 50 + 1, gridRow: anchor.y / 50 + 1 }"
              :title="$t('#L_LSO_ANCHOR_' + anchor.id.toUpperCase().replace('-', '_'))"
              :aria-label="$t('#L_LSO_ANCHOR_' + anchor.id.toUpperCase().replace('-', '_'))"
              :aria-pressed="overlay.x === anchor.x && overlay.y === anchor.y ? 'true' : 'false'"
              @click="set({ x: anchor.x, y: anchor.y })"
            ><span aria-hidden="true"></span></button>
          </div>
        </div>
        <p class="gx-row__hint">{{ $t('#L_LSO_POSITION_HINT') }}</p>
        <div v-for="axis in ['x', 'y']" :key="axis" class="gx-slider has-scale">
          <span class="gx-slider__end" aria-hidden="true">{{ $t(axis === 'x' ? '#L_LSO_LEFT' : '#L_LSO_TOP') }}</span>
          <span class="gx-slider__track" :class="{ 'is-default': !changed(axis) }" :style="sliderStyle(axis)">
            <span class="gx-slider__default" aria-hidden="true"></span>
            <input class="gx-range" type="range" min="0" max="100" step="0.5" :value="overlay[axis]" :aria-label="$t(axis === 'x' ? '#L_LSO_X' : '#L_LSO_Y')" :aria-valuetext="overlay[axis] + '%'" @input="set({ [axis]: Number($event.target.value) })" @dblclick="set({ [axis]: defaults[axis] })" />
          </span>
          <span class="gx-slider__end" aria-hidden="true">{{ $t(axis === 'x' ? '#L_LSO_RIGHT' : '#L_LSO_BOTTOM') }}</span>
        </div>
      </fieldset>

      <fieldset v-for="field in sizeFields" :key="field.key" class="gx-row gx-row--number" :class="{ 'is-changed': changed(field.key) }">
        <div class="gx-row__main">
          <label class="gx-row__label" :for="'lso-' + field.key">{{ $t(field.label) }}</label>
          <span class="gx-num has-unit has-step">
            <input :id="'lso-' + field.key" class="gx-input gx-input--num" type="number" :min="field.min" :max="field.max" :step="field.step" inputmode="numeric" :value="overlay[field.key]" autocomplete="off" @change="setNumber(field, $event.target)" />
            <span class="gx-num__unit" aria-hidden="true">%</span>
            <GxNumStep />
          </span>
        </div>
        <p v-if="field.hint" class="gx-row__hint">{{ $t(field.hint) }}</p>
        <div class="gx-slider">
          <span class="gx-slider__track" :class="{ 'is-default': !changed(field.key) }" :style="sliderStyle(field.key, field)">
            <span class="gx-slider__default" aria-hidden="true"></span>
            <input class="gx-range" type="range" :min="field.min" :max="field.max" :step="field.step" :value="overlay[field.key]" :aria-label="$t(field.label)" :aria-valuetext="overlay[field.key] + '%'" @input="set({ [field.key]: Number($event.target.value) })" @dblclick="set({ [field.key]: defaults[field.key] })" />
          </span>
        </div>
      </fieldset>

      <fieldset class="gx-row" :class="{ 'is-changed': changed('crop') }">
        <div class="gx-row__main">
          <span class="gx-row__label">{{ $t('#L_LSO_CROP') }}</span>
        </div>
        <p class="gx-row__hint">{{ $t('#L_LSO_CROP_HINT') }}</p>
        <div class="lso-crop">
          <label v-for="side in cropSides" :key="side" class="lso-crop__side">
            <span>{{ $t('#L_LSO_CROP_' + side.toUpperCase()) }}</span>
            <span class="gx-num has-unit has-step">
              <input class="gx-input gx-input--num" type="number" min="0" :max="limits.crop[1]" step="1" inputmode="numeric" :value="overlay.crop[side]" autocomplete="off" @change="setCrop(side, $event.target)" />
              <span class="gx-num__unit" aria-hidden="true">px</span>
              <GxNumStep />
            </span>
          </label>
        </div>
      </fieldset>

      <fieldset class="gx-row" :class="{ 'is-changed': overlay.key !== 'none' }">
        <div class="gx-row__main">
          <span class="gx-row__label">{{ $t('#L_LSO_KEY') }}</span>
        </div>
        <p class="gx-row__hint">{{ $t('#L_LSO_KEY_HINT') }}</p>
        <div class="lso-seg" role="radiogroup" :aria-label="$t('#L_LSO_KEY')">
          <button v-for="key in keys" :key="key" type="button" role="radio" class="lso-seg__item" :class="{ active: overlay.key === key }" :aria-checked="overlay.key === key ? 'true' : 'false'" @click="set({ key })">
            {{ $t('#L_LSO_KEY_' + key.toUpperCase()) }}
          </button>
        </div>
      </fieldset>

      <template v-if="overlay.key === 'color'">
        <fieldset class="gx-row gx-row--color" :class="{ 'is-changed': changed('keyColor') }">
          <div class="gx-row__main">
            <label class="gx-row__label" for="lso-key-color">{{ $t('#L_LSO_KEY_COLOR') }}</label>
            <span class="gx-color">
              <input id="lso-key-color" type="color" :value="hex(overlay.keyColor)" @input="setColor($event.target.value)" />
              <code>{{ hex(overlay.keyColor).toUpperCase() }}</code>
            </span>
          </div>
          <div class="lso-actions">
            <button type="button" class="gx-btn" :disabled="!backgroundColor" :title="$t('#L_LSO_PICK_BACKGROUND_HINT')" @click="set({ keyColor: backgroundColor })">
              <i class="fas fa-eye-dropper" aria-hidden="true"></i>{{ $t('#L_LSO_PICK_BACKGROUND') }}
            </button>
          </div>
        </fieldset>
        <fieldset v-for="field in colorFields" :key="field.key" class="gx-row gx-row--number" :class="{ 'is-changed': changed(field.key) }">
          <div class="gx-row__main">
            <label class="gx-row__label" :for="'lso-' + field.key">{{ $t(field.label) }}</label>
            <span class="gx-num has-step">
              <input :id="'lso-' + field.key" class="gx-input gx-input--num" type="number" :min="field.min" :max="field.max" :step="field.step" :value="overlay[field.key]" autocomplete="off" @change="setNumber(field, $event.target)" />
              <GxNumStep />
            </span>
          </div>
          <div class="gx-slider">
            <span class="gx-slider__track" :class="{ 'is-default': !changed(field.key) }" :style="sliderStyle(field.key, field)">
              <span class="gx-slider__default" aria-hidden="true"></span>
              <input class="gx-range" type="range" :min="field.min" :max="field.max" :step="field.step" :value="overlay[field.key]" :aria-label="$t(field.label)" @input="set({ [field.key]: Number($event.target.value) })" @dblclick="set({ [field.key]: defaults[field.key] })" />
            </span>
          </div>
        </fieldset>
      </template>

      <template v-if="overlay.key === 'luma'">
        <fieldset v-for="field in lumaFields" :key="field.key" class="gx-row gx-row--number" :class="{ 'is-changed': changed(field.key) }">
          <div class="gx-row__main">
            <label class="gx-row__label" :for="'lso-' + field.key">{{ $t(field.label) }}</label>
            <span class="gx-num has-step">
              <input :id="'lso-' + field.key" class="gx-input gx-input--num" type="number" min="0" max="1" step="0.01" inputmode="decimal" :value="overlay[field.key]" autocomplete="off" @change="setNumber(field, $event.target)" />
              <GxNumStep />
            </span>
          </div>
          <div class="gx-slider">
            <span class="gx-slider__track" :class="{ 'is-default': !changed(field.key) }" :style="sliderStyle(field.key, field)">
              <span class="gx-slider__default" aria-hidden="true"></span>
              <input class="gx-range" type="range" min="0" max="1" step="0.01" :value="overlay[field.key]" :aria-label="$t(field.label)" @input="set({ [field.key]: Number($event.target.value) })" @dblclick="set({ [field.key]: defaults[field.key] })" />
            </span>
          </div>
        </fieldset>
      </template>
    </div>
  </section>
</template>

<script>
import GxNumStep from './GxNumStep';
const overlaySettings = require('../../../shared/livesplit-overlay');

const SIZE_FIELDS = [
  { key: 'height', label: '#L_LSO_HEIGHT', hint: '#L_LSO_HEIGHT_HINT', min: 5, max: 100, step: 1 },
  { key: 'opacity', label: '#L_LSO_OPACITY', min: 0, max: 100, step: 1 },
];
const COLOR_FIELDS = [
  { key: 'similarity', label: '#L_LSO_SIMILARITY', min: 1, max: 1000, step: 1 },
  { key: 'smoothness', label: '#L_LSO_SMOOTHNESS', min: 1, max: 1000, step: 1 },
];
const LUMA_FIELDS = [
  { key: 'lumaMin', label: '#L_LSO_LUMA_MIN', min: 0, max: 1 },
  { key: 'lumaMinSmooth', label: '#L_LSO_LUMA_MIN_SMOOTH', min: 0, max: 1 },
  { key: 'lumaMax', label: '#L_LSO_LUMA_MAX', min: 0, max: 1 },
  { key: 'lumaMaxSmooth', label: '#L_LSO_LUMA_MAX_SMOOTH', min: 0, max: 1 },
];
// Снимок под Windows быстрый, его можно обновлять сам; под Proton каждый
// снимок — отдельная сессия umu на несколько секунд, там только по кнопке.
const AUTO_REFRESH_MS = 2000;
const MIN_SPIN_MS = 600;

export default {
  name: 'LiveSplitOverlayEditor',
  components: { GxNumStep },
  emits: ['input', 'snapshot'],
  props: {
    value: { type: Object, default: null },
    gameId: { type: String, required: true },
    gameTitle: { type: String, default: '' },
    active: { type: Boolean, default: true },
  },
  data() {
    return {
      anchors: overlaySettings.ANCHORS.concat({ id: 'center', x: 50, y: 50 }),
      keys: overlaySettings.KEYS,
      limits: overlaySettings.LIMITS,
      defaults: overlaySettings.defaultOverlay(),
      sizeFields: SIZE_FIELDS,
      colorFields: COLOR_FIELDS,
      lumaFields: LUMA_FIELDS,
      cropSides: ['top', 'right', 'bottom', 'left'],
      capturing: false,
      flash: false,
      result: null,
      capturedAt: 0,
    };
  },
  computed: {
    overlay() { return overlaySettings.sanitizeOverlay(this.value); },
    changes() { return overlaySettings.overlayChanges(this.value); },
    resetLabel() { return this.$t('#L_GS_X_RESET_SECTION') + (this.changes ? ' — ' + this.$t('#L_GS_X_CHANGED', this.changes) : ''); },
    linux() { return window.hlsrElectron && window.hlsrElectron.platform === 'linux'; },
    status() { return this.result ? this.result.status : 'pending'; },
    // LiveSplit запущен для другой игры: на Linux он в чужом префиксе, на
    // Windows оверлей показывает его только в той игре.
    otherPrefix() { return !!this.result && this.result.otherGame === true; },
    sourceTone() { return this.status === 'ok' && !this.otherPrefix ? 'ok' : this.status === 'error' || this.otherPrefix ? 'warn' : 'idle'; },
    sourceIcon() { return this.status === 'ok' && !this.otherPrefix ? 'fa-check-circle' : this.sourceTone === 'warn' ? 'fa-exclamation-triangle' : 'fa-info-circle'; },
    sourceTitle() {
      if (this.capturing && !this.result) return '#L_LSO_SOURCE_CAPTURING';
      if (this.otherPrefix) return '#L_LSO_SOURCE_OTHER_GAME';
      return {
        ok: '#L_LSO_SOURCE_LIVE',
        'not-running': '#L_LSO_SOURCE_SAMPLE',
        'no-window': '#L_LSO_SOURCE_NO_WINDOW',
        error: '#L_LSO_SOURCE_ERROR',
      }[this.status] || '#L_LSO_SOURCE_SAMPLE';
    },
    sourceHint() {
      if (this.otherPrefix) return this.$t('#L_LSO_SOURCE_OTHER_GAME_HINT');
      if (this.status === 'ok') return this.$t('#L_LSO_SOURCE_LIVE_HINT', this.result.width + '×' + this.result.height);
      if (this.status === 'no-window') return this.$t('#L_LSO_SOURCE_NO_WINDOW_HINT');
      if (this.status === 'error') return this.$t('#L_LSO_SOURCE_ERROR_HINT');
      return this.$t('#L_LSO_SOURCE_SAMPLE_HINT');
    },
    // Цвет фона LiveSplit для цветового ключа.
    backgroundColor() {
      const result = this.result;
      if (!result || result.status !== 'ok' || !result.pixels) return null;
      return overlaySettings.backgroundColor(result.pixels, result.width, result.height);
    },
  },
  watch: {
    active: { immediate: true, handler(value) { if (value) this.capture(); this.schedule(); } },
  },
  beforeUnmount() { clearTimeout(this.timer); this.gone = true; },
  methods: {
    schedule() {
      clearTimeout(this.timer);
      if (!this.active || this.linux || this.gone) return;
      this.timer = setTimeout(() => { this.capture(true).then(() => this.schedule()); }, AUTO_REFRESH_MS);
    },
    // Кнопка «Обновить»: снимок часто готов за десятки миллисекунд и
    // ничего не меняет (LiveSplit не запущен), поэтому значок крутится
    // не меньше MIN_SPIN_MS, а карточка коротко подсвечивается.
    async refresh() {
      await this.capture(false, true);
      if (this.gone) return;
      this.flash = false;
      requestAnimationFrame(() => { if (!this.gone) this.flash = true; });
    },
    async capture(quiet, manual) {
      if (this.capturing || !window.hlsrElectron) return;
      this.capturing = !quiet;
      const started = Date.now();
      try {
        const result = await window.hlsrElectron.ipcRenderer.invoke('livesplit-overlay-capture', this.gameId);
        if (this.gone) return;
        // Пока LiveSplit на секунду пропал (свернули), прежний снимок лучше
        // пустого примера.
        if (quiet && result && result.status !== 'ok' && this.result && this.result.status === 'ok') return;
        this.result = result || { status: 'error' };
        this.capturedAt = Date.now();
        this.$emit('snapshot', this.result.status === 'ok' ? this.result : null);
      } catch (error) {
        if (!this.gone) { this.result = { status: 'error' }; this.$emit('snapshot', null); }
      } finally {
        const rest = manual ? MIN_SPIN_MS - (Date.now() - started) : 0;
        if (rest > 0) await new Promise((resolve) => setTimeout(resolve, rest));
        this.capturing = false;
      }
    },
    changed(key) { return JSON.stringify(this.overlay[key]) !== JSON.stringify(this.defaults[key]); },
    set(patch) { this.$emit('input', overlaySettings.sanitizeOverlay(Object.assign({}, this.overlay, patch))); },
    setNumber(field, input) {
      const next = overlaySettings.sanitizeOverlay(Object.assign({}, this.overlay, { [field.key]: input.value }));
      input.value = String(next[field.key]);
      this.$emit('input', next);
    },
    setCrop(side, input) {
      const next = overlaySettings.sanitizeOverlay(Object.assign({}, this.overlay, { crop: Object.assign({}, this.overlay.crop, { [side]: input.value }) }));
      input.value = String(next.crop[side]);
      this.$emit('input', next);
    },
    setColor(value) {
      if (!/^#[0-9a-f]{6}$/i.test(value)) return;
      this.set({ keyColor: { r: parseInt(value.slice(1, 3), 16), g: parseInt(value.slice(3, 5), 16), b: parseInt(value.slice(5, 7), 16) } });
    },
    hex(color) { return '#' + ['r', 'g', 'b'].map((key) => color[key].toString(16).padStart(2, '0')).join(''); },
    sliderStyle(key, field) {
      const min = field ? field.min : 0, max = field ? field.max : 100;
      const at = (v) => Math.max(0, Math.min(100, (Number(v) - min) / (max - min) * 100));
      const value = at(this.overlay[key]), base = at(this.defaults[key]);
      return { '--value': value + '%', '--default': base + '%', '--from': Math.min(value, base) + '%', '--to': Math.max(value, base) + '%' };
    },
    // Сброс раздела не выключает оверлей: выключатель — отдельное решение.
    reset() { this.$emit('input', Object.assign(overlaySettings.defaultOverlay(), { enabled: this.overlay.enabled })); },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

/* Иконка слева, текст и кнопка под ним одной колонкой: рядом с кнопкой
   подсказка в узкой панели ломалась на короткие строки. */
.lso-source {
  display: grid;
  grid-template-columns: 16px minmax(0, 1fr);
  align-items: start;
  gap: 10px 12px;
  margin: 14px 14px 8px;
  padding: 14px 16px;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm, 10px);
  background: rgba(255, 255, 255, 0.03);
  font-size: 12px;
  line-height: 1.4;
  > i { width: 16px; padding-top: 2px; font-size: 14px; text-align: center; color: var(--l-text-faint); }
  &.is-ok > i { color: var(--accent); }
  &.is-warn { border-color: rgba(255, 184, 77, 0.35); }
  &.is-warn > i { color: #ffb84d; }
  &__text { display: flex; flex-direction: column; gap: 4px; min-width: 0; color: var(--l-text-faint); text-wrap: pretty; }
  &__text strong { color: var(--l-text); font-size: 13px; font-weight: 600; }
  &__refresh { grid-column: 2; justify-self: start; }
  // Пока идёт снимок, кнопка не бледнеет: крутящийся значок и есть ответ
  &__refresh:disabled { opacity: 1; cursor: progress; }
  &.is-flash { animation: lso-source-flash 600ms var(--l-ease, ease-out); }
}
@keyframes lso-source-flash {
  0% { border-color: color-mix(in srgb, var(--accent) 70%, transparent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
  100% { border-color: var(--l-border); background: rgba(255, 255, 255, 0.03); }
}
@media (prefers-reduced-motion: reduce) {
  .lso-source.is-flash { animation-duration: 1ms; }
}

.lso-anchors {
  display: grid;
  flex: none;
  grid-template-columns: repeat(3, 22px);
  grid-template-rows: repeat(3, 16px);
  gap: 3px;
  padding: 4px;
  border: 1px solid var(--l-border);
  border-radius: 7px;
  background: rgba(0, 0, 0, 0.25);
  &__cell {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    border: 0;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.06);
    cursor: pointer;
    transition: background var(--l-fast);
    span { width: 6px; height: 6px; border-radius: 50%; background: var(--l-text-faint); transition: background var(--l-fast), transform var(--l-fast); }
    &:hover { background: rgba(255, 255, 255, 0.12); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
    &.active { background: var(--accent-soft); }
    &.active span { background: var(--accent); transform: scale(1.3); }
  }
}

.lso-crop {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 6px 14px;
  &__side { display: flex; align-items: center; justify-content: space-between; gap: 8px; color: var(--l-text-dim); font-size: 12.5px; }
  .gx-input--num { width: 92px; }
}

.lso-seg {
  display: flex;
  gap: 2px;
  padding: 3px;
  border: 1px solid var(--l-border);
  border-radius: 9px;
  background: rgba(0, 0, 0, 0.2);
  &__item {
    flex: 1;
    min-width: 0;
    padding: 6px 8px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);
    &:hover { color: var(--l-text); background: rgba(255, 255, 255, 0.05); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
    &.active { background: var(--accent-soft); color: var(--l-text); }
  }
}

.lso-actions { display: flex; justify-content: flex-end; }
.lso-source .gx-btn i, .lso-actions .gx-btn i { margin-right: 6px; }
</style>
