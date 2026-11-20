<template>
  <div class="gx gx--no-nav" :class="{ 'gx--wide': !hasPreview }" :style="fieldsWidth ? { '--gx-fields-w': fieldsWidth + 'px' } : null">
    <GhostingPersonalization
      v-show="isPersonalization"
      :id="isPersonalization ? 'gx-panel' : null"
      role="tabpanel"
      aria-labelledby="gx-tab-personalization"
      :value="value"
      :goldsrc="!!schema"
      @input="$emit('input', $event)"
      @panel="personalizationPanel = $event"
    />

    <div v-if="activeId === 'settings-code'" id="gx-panel" class="gx-code" role="tabpanel" aria-labelledby="gx-tab-settings-code">
      <slot name="settings-code"></slot>
    </div>

    <LiveSplitOverlayEditor
      v-else-if="isLiveSplit"
      :value="overlay"
      :game-id="gameId"
      :active="active"
      @input="$emit('overlay', $event)"
      @snapshot="liveSplitSnapshot = $event"
    />

    <CustomPacks v-else-if="activeId === MODS.id" :game-id="gameId" :title="activeSection.title" @packs="modPacks = $event" />

    <section v-else-if="!isPersonalization && !isLiveSplit" id="gx-panel" class="gx-pane" :class="{ 'gx-pane--packs': activeId === 'hl1packs' }" role="tabpanel" :aria-labelledby="'gx-tab-' + activeId">
      <header class="gx-pane__head">
        <h3 class="gx-pane__title">{{ $t(activeSection.title) }}</h3>
        <button type="button" class="gx-btn gx-pane__reset" :disabled="!sectionChanges(activeSection)" :title="$t('#L_GS_X_RESET_HINT')" :aria-label="resetLabel" @click="resetSection">
          <i class="fas fa-undo-alt" aria-hidden="true"></i>
          <b v-if="sectionChanges(activeSection)" class="gx-btn__count" aria-hidden="true">{{ sectionChanges(activeSection) }}</b>
        </button>
        <p class="gx-pane__note" :title="$t('#L_GS_X_NOTE_' + activeId.toUpperCase())">{{ $t('#L_GS_X_NOTE_' + activeId.toUpperCase()) }}</p>
        <button v-if="activeSection.link" type="button" class="gx-btn gx-pane__link" @click="openLink(activeSection.link)">
          <i class="fas fa-external-link-alt" aria-hidden="true"></i>{{ $t('#L_GS_X_LINK_' + activeId.toUpperCase()) }}
        </button>
      </header>

      <div ref="fields" class="gx-fields">
        <div v-for="(column, columnIndex) in fieldColumns" :key="columnIndex" class="gx-col">
          <fieldset v-for="field in column" :key="field.key" class="gx-row" :class="['gx-row--' + field.type, { 'is-changed': isChanged(field), 'is-off': !isAvailable(field) }]" :disabled="!isAvailable(field)">
            <div class="gx-row__main">
              <label class="gx-row__label" :for="inputId(field)">{{ $t(field.label) }}</label>

              <label v-if="field.type === 'boolean'" class="gx-switch">
                <input :id="inputId(field)" type="checkbox" :name="field.key" :checked="!!resolved[field.key]" :aria-describedby="hintId(field)" @change="set(field, $event.target.checked)" />
                <span class="gx-switch__track" aria-hidden="true"><span class="gx-switch__thumb"></span></span>
              </label>

              <span v-else-if="field.type === 'number'" class="gx-num has-step" :class="{ 'has-unit': !!field.unit }">
              <input
                :id="inputId(field)"
                class="gx-input gx-input--num"
                type="number"
                :name="field.key"
                :min="field.min"
                :max="field.max"
                :step="field.step || 1"
                :inputmode="(field.step || 1) < 1 ? 'decimal' : 'numeric'"
                :value="resolved[field.key]"
                :aria-describedby="hintId(field)"
                autocomplete="off"
                @change="setNumber(field, $event.target.value, $event.target)"
              />
              <span v-if="field.unit" class="gx-num__unit" aria-hidden="true">{{ field.unit }}</span>
              <GxNumStep />
              </span>

              <!-- Свой список вместо системного <select>: выпадающий список
                   Chromium не красится под лаунчер. -->
              <UiDropdown
                v-else-if="field.type === 'select'"
                class="gx-dropdown"
                :toggle-id="inputId(field)"
                :described-by="hintId(field)"
                :items="selectItems(field)"
                :value="resolved[field.key]"
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

              <span v-else-if="field.type === 'color'" class="gx-color">
                <input :id="inputId(field)" type="color" :name="field.key" :value="toHex(resolved[field.key])" :aria-describedby="hintId(field)" @input="setColor(field, $event.target.value)" />
                <code>{{ toHex(resolved[field.key]).toUpperCase() }}</code>
              </span>
            </div>

            <p v-if="field.hint" :id="hintId(field)" class="gx-row__hint">{{ $t(field.hint) }}</p>

            <div v-if="field.type === 'number' && hasRange(field)" class="gx-slider" :class="{ 'has-scale': !!field.scale }">
              <span v-if="field.scale" class="gx-slider__end" aria-hidden="true">{{ $t(field.scale[0]) }}</span>
              <span class="gx-slider__track" :class="{ 'is-default': !isChanged(field) }" :style="sliderStyle(field)">
                <span class="gx-slider__default" aria-hidden="true"></span>
                <input
                  class="gx-range"
                  type="range"
                  :min="field.min"
                  :max="field.max"
                  step="any"
                  :value="sliderValue(field)"
                  :aria-label="$t(field.label)"
                  :aria-valuetext="resolved[field.key] + (field.unit || '')"
                  :title="$t('#L_GS_X_SLIDER_DEFAULT', field.default + (field.unit || ''))"
                  @input="slide(field, $event.target)"
                  @change="endSlide(field, $event.target)"
                  @dblclick="set(field, field.default)"
                />
              </span>
              <span v-if="field.scale" class="gx-slider__end" aria-hidden="true">{{ $t(field.scale[1]) }}</span>
            </div>

            <div v-else-if="field.type === 'text'" class="gx-text">
              <input
                :id="inputId(field)"
                class="gx-input"
                :class="{ invalid: invalidFields[field.key] }"
                type="text"
                :name="field.key"
                :value="resolved[field.key]"
                :maxlength="field.maxLength || 240"
                :placeholder="$t('#L_GS_X_EMPTY_VALUE')"
                spellcheck="false"
                autocomplete="off"
                :aria-invalid="invalidFields[field.key] ? 'true' : null"
                :aria-describedby="invalidFields[field.key] ? inputId(field) + '-error' : hintId(field)"
                @input="set(field, $event.target.value)"
              />
              <button v-if="assetKind(field)" type="button" class="gx-btn gx-btn--icon" :disabled="!!pickingAsset" :title="$t('#L_GS_X_BROWSE')" :aria-label="$t('#L_GS_X_BROWSE') + ': ' + $t(field.label)" @click="pickAsset(field)">
                <i class="fas" :class="pickingAsset === field.key ? 'fa-spinner fa-spin' : 'fa-folder-open'" aria-hidden="true"></i>
              </button>
            </div>

            <p v-if="invalidFields[field.key]" :id="inputId(field) + '-error'" class="gx-row__error" role="alert">{{ $t(field.key === 'gh_chat_connect' ? '#L_GS_X_INVALID_YOUTUBE' : field.key === 'gh_chat_channel' ? '#L_GS_X_INVALID_TWITCH' : '#L_GS_X_INVALID_TEXT') }}</p>
            <p v-if="assetErrors[field.key]" class="gx-row__error" role="alert">{{ $t(assetErrors[field.key] === true ? '#L_GS_X_PICK_ERROR' : assetErrors[field.key]) }}</p>
            <p v-if="!isAvailable(field)" class="gx-row__needs">{{ $t('#L_GS_X_REQUIRES', $t(requirementLabel(field))) }}</p>
          </fieldset>
        </div>
      </div>
    </section>

    <!-- Ширина колонки настроек: одна на все разделы с предпросмотром игры. -->
    <PaneSplitter v-if="hasPreview" v-model="fieldsWidth" :storage-key="'pz-fields:' + gameId" :min="340" :rest="400" label="#L_SPLIT_FIELDS" />

    <GhostingSettingsPreview
      v-show="hasPreview"
      class="gx-stage"
      :value="value"
      :section="previewSection"
      :game-id="gameId"
      :variant="schema ? 'hl1' : 'ghosting'"
      :active="active && hasPreview"
      :overlay="overlay"
      :overlay-snapshot="liveSplitSnapshot"
      :overlay-editing="isLiveSplit"
      :hud-mod="hudMod"
      @overlay="$emit('overlay', $event)"
    />
  </div>
</template>

<script>
import GhostingPersonalization from './GhostingPersonalization';
import GhostingSettingsPreview from './GhostingSettingsPreview';
import UiDropdown from './UiDropdown';
import GxNumStep from './GxNumStep';
import LiveSplitOverlayEditor from './LiveSplitOverlayEditor';
import CustomPacks from './CustomPacks';
import PaneSplitter from './PaneSplitter';
import { personalizationChanges } from './ghosting-personalization-defaults';
import { overlayChanges } from '../../../shared/livesplit-overlay';
const GHOSTING_SCHEMA = require('../../../shared/ghosting-settings');

// HUD colour and crosshair are not mod cvars: they have their own editor, but
// live in the same list so all the HUD settings are in one place.
const PERSONALIZATION = { id: 'personalization', title: '#L_GS_S_LOOK', fields: [] };
const SETTINGS_CODE = { id: 'settings-code', title: '#L_PZ_CODE_TITLE', icon: 'fa-share-alt', fields: [] };
// Оверлей LiveSplit в окне игры: свой редактор, свои настройки (не extra).
const LIVESPLIT = { id: 'livesplit', title: '#L_LSO_TITLE', icon: 'fa-layer-group', fields: [] };
// Моды игры (CustomPacks.vue): карточки с переключателем, включаются сразу, без «Сохранить».
const MODS = { id: 'mods', icon: 'fa-cubes', fields: [] };
// Only these sections change something in the preview frame. The others use
// the full width for their settings instead. «Призраки» и «Эффекты и спреи»
// показывают свою сцену (ghosting-scene-renderer), остальные — кадр с HUD.
const PREVIEW_SECTIONS = ['personalization', 'hud', 'layout', 'keys', 'weapon', 'meters', 'ghosts', 'misc', 'livesplit'];
const HL1_PREVIEW_SECTIONS = ['personalization', 'hl1packs', 'hl1hud', 'hl1view', 'livesplit'];
const ICONS = { personalization: 'fa-palette', hud: 'fa-font', layout: 'fa-th-large', keys: 'fa-keyboard', weapon: 'fa-crosshairs', meters: 'fa-stopwatch', ghosts: 'fa-ghost', chat: 'fa-comment-alt', misc: 'fa-sliders-h' };
// How close to the default mark, in pixels, a slider thumb sticks to it.
const SLIDER_DETENT_PX = 6;
const same = (a, b) => (a && typeof a === 'object' ? !!b && a.r === b.r && a.g === b.g && a.b === b.b : a === b);

export default {
  name: 'GhostingSettingsExtras',
  components: { GhostingPersonalization, GhostingSettingsPreview, GxNumStep, CustomPacks, LiveSplitOverlayEditor, PaneSplitter, UiDropdown },
  emits: ["input", "validity", "overlay", "nav"],
  props: {
    active: { type: Boolean, default: true },
    // Список подразделов ведёт страница «Персонализация» (пунктом слева): подраздел приходит в section,
    // а список и выбранный уходят событием nav.
    section: { type: String, default: '' },
    value: { type: Object, required: true }, gameId: { type: String, default: '218' },
    // Поля другой игры в том же формате (HL1: src/shared/goldsrc-settings).
    // У Half-Life свои кадры предпросмотра и спрайты паков.
    schema: { type: Object, default: null },
    // Настройки оверлея LiveSplit (src/shared/livesplit-overlay). Без них
    // раздела нет: окно настроек игры его не показывает.
    overlay: { type: Object, default: null },
  },
  data() { return { MODS, modPacks: null, activeId: this.section || PERSONALIZATION.id, personalizationPanel: 'hud', pickingAsset: '', assetErrors: {}, sliding: {}, liveSplitSnapshot: null, fieldsWidth: null }; },
  beforeUnmount() { if (this.slideFrame) cancelAnimationFrame(this.slideFrame); },
  // Включён ли HUD-мод, предпросмотру нужно знать и без открытой вкладки модов.
  mounted() {
    if (this.schema || !window.hlsrElectron) return;
    window.hlsrElectron.ipcRenderer.invoke('custom-packs-get', this.gameId)
      .then((result) => { if (result && result.status === 'ok' && !this.modPacks) this.modPacks = result.packs; }).catch(() => {});
  },
  computed: {
    fieldSchema() { return this.schema || GHOSTING_SCHEMA; },
    navItems() { return [PERSONALIZATION].concat(this.fieldSchema.SECTIONS, [Object.assign({ title: this.schema ? '#L_CPK_TAB_HL1' : '#L_CPK_TAB_GHOSTING' }, MODS)], this.overlay ? [LIVESPLIT] : [], this.$slots['settings-code'] ? [SETTINGS_CODE] : []); },
    isLiveSplit() { return this.activeId === LIVESPLIT.id; },
    // HUD-мод Ghosting включён: предпросмотр рисует его HUD (ghosting-overlay-renderer, CONDENSED).
    hudMod() { return !this.schema && (this.modPacks || []).some((pack) => pack.id === 'condensed-hud' && pack.enabled) ? 'condensed-hud' : ''; },
    activeSection() { return this.navItems.find((section) => section.id === this.activeId) || this.navItems[0]; },
    isPersonalization() { return this.activeId === PERSONALIZATION.id; },
    hasPreview() { return this.schema ? HL1_PREVIEW_SECTIONS.includes(this.activeId) : PREVIEW_SECTIONS.includes(this.activeId); },
    // The personalization editor shows either the HUD colour or the crosshair.
    previewSection() { return this.isPersonalization ? this.personalizationPanel : this.activeId; },
    // Without a preview the settings use the full width as two independent
    // columns, so a short setting next to a tall one leaves no gap.
    fieldColumns() {
      // Форма и цвет нужны только выбранному Crosshair Changer. Значения
      // сохраняются в черновике и возвращаются при повторном выборе пака.
      const fields = this.activeSection.fields.filter((field) => this.activeId !== 'hl1packs' || !field.requiresValue || this.isAvailable(field));
      if (this.hasPreview) return [fields];
      const middle = Math.ceil(fields.length / 2);
      return [fields.slice(0, middle), fields.slice(middle)];
    },
    extra() { return this.value.extra || {}; },
    resolved() { return Object.assign(this.fieldSchema.defaults(), this.extra); },
    invalidFields() {
      const sanitized = this.fieldSchema.sanitizeExtras(this.extra);
      return this.fieldSchema.FIELDS.reduce((invalid, field) => {
        if (this.isOverridden(field.key) && !Object.prototype.hasOwnProperty.call(sanitized, field.key)) invalid[field.key] = true;
        return invalid;
      }, {});
    },
    valid() { return Object.keys(this.invalidFields).length === 0; },
    // Для списка на странице: label — ключ перевода, changes — сколько настроек изменено.
    navState() {
      return { active: this.activeSection.id, items: this.navItems.map((section) => ({ id: section.id, icon: this.sectionIcon(section.id), label: section.title, changes: this.sectionChanges(section) })) };
    },
    resetLabel() {
      const count = this.sectionChanges(this.activeSection);
      return this.$t('#L_GS_X_RESET_SECTION') + (count ? ' — ' + this.$t('#L_GS_X_CHANGED', count) : '');
    },
  },
  watch: {
    valid: { immediate: true, handler(value) { this.$emit('validity', value); } },
    section(id) { if (id && id !== this.activeId) this.selectSection(id); },
    navState: { immediate: true, handler(value) { this.$emit('nav', value); } },
  },
  methods: {
    sectionIcon(id) { const section = this.navItems.find((item) => item.id === id); return 'fas ' + ((section && section.icon && section.icon.startsWith('fa-') ? section.icon : ICONS[id]) || 'fa-cog'); },
    inputId(field) { return 'ghost-extra-' + field.key; },
    selectItems(field) { return field.options.map((option) => ({ id: option.value, label: this.$t(option.label) })); },
    selectLabel(field) { const found = field.options.find((option) => option.value === this.resolved[field.key]); return found ? this.$t(found.label) : String(this.resolved[field.key] == null ? '' : this.resolved[field.key]); },
    hintId(field) { return field.hint ? this.inputId(field) + '-hint' : null; },
    // Off while the setting it depends on is off; the value is kept and still written.
    // A field that needs a particular choice names that choice, not the field.
    requirementLabel(field) {
      if (field.requiresValue === undefined) return this.fieldLabel(field.requires);
      const parent = this.fieldSchema.FIELDS.find((item) => item.key === field.requires);
      const choice = parent && parent.options && parent.options.find((item) => item.value === field.requiresValue);
      return choice ? choice.label : this.fieldLabel(field.requires);
    },
    isAvailable(field) { return this.fieldSchema.available(field, this.resolved); },
    fieldLabel(key) { const field = this.fieldSchema.FIELDS.find((item) => item.key === key); return field ? field.label : key; },
    isOverridden(key) { return Object.prototype.hasOwnProperty.call(this.extra, key); },
    // A setting counts as changed when it is written by the launcher with a
    // value other than the game's default.
    isChanged(field) { return this.isOverridden(field.key) && !same(this.resolved[field.key], field.default); },
    sectionChanges(section) {
      if (section.id === PERSONALIZATION.id) return personalizationChanges(this.value);
      if (section.id === LIVESPLIT.id) return overlayChanges(this.overlay);
      return section.fields.filter((field) => this.isChanged(field)).length;
    },
    // Both axes of a layout offset are 960 to 1280 units wide; they deserve a slider too.
    // Where the value and the default sit on the track, 0 to 100 %. The fill
    // runs from the default to the value, so an offset reads as "this far from
    // where it was".
    sliderStyle(field) {
      const at = (v) => Math.max(0, Math.min(100, (Number(v) - field.min) / (field.max - field.min) * 100));
      const value = at(this.sliderValue(field)), base = at(field.default);
      return { '--value': value + '%', '--default': base + '%', '--from': Math.min(value, base) + '%', '--to': Math.max(value, base) + '%' };
    },
    // While a slider is dragged its thumb follows the pointer freely; the
    // setting takes the nearest step, at most once a frame, so the preview and
    // the form keep up. Letting go snaps the thumb to the step.
    sliderValue(field) { return Object.prototype.hasOwnProperty.call(this.sliding, field.key) ? this.sliding[field.key] : this.resolved[field.key]; },
    slide(field, input) {
      const raw = this.detent(field, input);
      this.sliding[field.key] = raw;
      this.pendingSlide = { field, raw };
      if (this.slideFrame) return;
      this.slideFrame = requestAnimationFrame(() => { this.slideFrame = 0; const next = this.pendingSlide; this.pendingSlide = null; if (next) this.setNumber(next.field, this.snap(next.field, next.raw)); });
    },
    endSlide(field, input) {
      if (this.slideFrame) { cancelAnimationFrame(this.slideFrame); this.slideFrame = 0; this.pendingSlide = null; }
      delete this.sliding[field.key];
      this.setNumber(field, this.snap(field, this.detent(field, input)));
    },
    // The default mark is a detent: within a few pixels of it the thumb
    // sticks to the default. A step of a fine slider is about a pixel wide,
    // so landing exactly on 0 by hand would otherwise take luck.
    detent(field, input) {
      const raw = Number(input.value), width = input.offsetWidth;
      if (!width || !Number.isFinite(Number(field.default))) return raw;
      return Math.abs(raw - field.default) / (field.max - field.min) * width <= SLIDER_DETENT_PX ? Number(field.default) : raw;
    },
    snap(field, raw) {
      const step = field.step || 1, value = field.min + Math.round((Number(raw) - field.min) / step) * step;
      return Number(value.toFixed(6));
    },
    hasRange(field) { return Number.isFinite(field.min) && Number.isFinite(field.max) && field.max - field.min <= 1500; },
    selectSection(id) { this.activeId = id; this.$nextTick(() => { if (this.$refs.fields) this.$refs.fields.scrollTop = 0; }); },
    set(field, nextValue) { delete this.assetErrors[field.key]; this.$emit('input', Object.assign({}, this.value, { extra: Object.assign({}, this.extra, { [field.key]: nextValue }) })); },
    assetKind(field) { return field.key === 'gh_spray_file' ? 'spray' : field.key === 'gh_hud_font' ? 'font' : field.key === 'gh_model' ? 'model' : ''; },
    async pickAsset(field) {
      if (this.pickingAsset) return;
      this.pickingAsset = field.key;
      delete this.assetErrors[field.key];
      try {
        const result = await window.hlsrElectron.ipcRenderer.invoke('game-personalization-pick-asset', this.gameId, this.assetKind(field));
        if (result && result.status === 'ok') this.set(field, result.value);
        else if (result && result.status !== 'canceled') this.assetErrors[field.key] = result.code === 'ASSET_OUTSIDE_MOD' ? '#L_GS_X_MODEL_OUTSIDE' : true;
      } catch (error) { this.assetErrors[field.key] = true; }
      finally { this.pickingAsset = ''; }
    },
    setNumber(field, value, input) {
      const clean = this.fieldSchema.sanitizeExtras({ [field.key]: value });
      if (!Object.prototype.hasOwnProperty.call(clean, field.key)) {
        if (input) input.value = String(this.resolved[field.key]);
        return;
      }
      if (input) input.value = String(clean[field.key]);
      this.set(field, clean[field.key]);
    },
    toHex(color) {
      const source = color && typeof color === 'object' ? color : { r: 255, g: 220, b: 0 };
      return '#' + ['r', 'g', 'b'].map((key) => Math.max(0, Math.min(255, Math.round(Number(source[key]) || 0))).toString(16).padStart(2, '0')).join('');
    },
    setColor(field, value) {
      if (!/^#[0-9a-f]{6}$/i.test(value)) return;
      this.set(field, { r: parseInt(value.slice(1, 3), 16), g: parseInt(value.slice(3, 5), 16), b: parseInt(value.slice(5, 7), 16) });
    },
    openLink(key) { window.hlsrElectron.ipcRenderer.invoke('open-external-link', key); },
    resetSection() {
      const extra = Object.assign({}, this.extra);
      this.activeSection.fields.forEach((field) => {
        delete this.assetErrors[field.key];
        if (this.isOverridden(field.key)) extra[field.key] = typeof field.default === 'object' ? Object.assign({}, field.default) : field.default;
      });
      this.$emit('input', Object.assign({}, this.value, { extra }));
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

/* Список подразделов слева живёт на странице, здесь колонок на одну меньше. Вместо зазора между
   полями и предпросмотром — PaneSplitter (12 px). Ширина из окна побольше зажимается: полям не меньше
   340 px (уже не влезают R, G, B цвета), предпросмотру — 400 px (как :min и :rest разделителя). */
.gx--no-nav {
  grid-template-columns: clamp(340px, var(--gx-fields), calc(100% - 412px)) 12px minmax(0, 1fr);
  column-gap: 0;
}
.gx--no-nav.gx--wide { grid-template-columns: minmax(0, 1fr); }
</style>
