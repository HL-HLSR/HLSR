<template>
  <section class="pers">
    <div class="pers__tabs" role="tablist">
      <button type="button" role="tab" :aria-selected="activePanel === 'hud'" :class="{ active: activePanel === 'hud' }" @click="activePanel = 'hud'">
        <i class="fas fa-tint" aria-hidden="true"></i>{{ $t("#L_GS_P_HUD") }}
      </button>
      <button type="button" role="tab" :aria-selected="activePanel === 'cross'" :class="{ active: activePanel === 'cross' }" @click="activePanel = 'cross'">
        <i class="fas fa-crosshairs" aria-hidden="true"></i>{{ $t("#L_GS_P_CROSS") }}
      </button>
    </div>

    <div class="pers__panel-scroll">
      <div v-show="activePanel === 'hud'" class="pers__panel" role="tabpanel">
        <div class="pers__feature-head">
          <div>
            <strong>{{ $t("#L_GS_P_HUD_TITLE") }}</strong>
            <span>{{ $t(goldsrc ? "#L_GS_P_HUD_HINT_GOLDSRC" : "#L_GS_P_HUD_HINT") }}</span>
          </div>
          <label class="settings-switch">
            <input type="checkbox" :checked="value.hudEnabled" :aria-label="$t('#L_GS_P_HUD_TITLE')" @change="set('hudEnabled', $event.target.checked)" />
            <span class="settings-switch__track"><span class="settings-switch__thumb"></span></span>
          </label>
        </div>

        <div class="pers__group">
          <span class="pers__label">{{ $t("#L_GS_P_PALETTE") }}</span>
          <div class="pers__swatches">
            <button v-for="p in hudPresets" :key="p.id" type="button" class="pers__swatch" :class="{ active: isHudPreset(p) }" :style="{ background: p.css }" :title="p.label" :aria-label="p.label" @click="setHudPreset(p)"></button>
            <label class="pers__native" :title="$t('#L_GS_P_PICK')">
              <input type="color" :value="hudHex" :aria-label="$t('#L_GS_P_PICK')" @input="onHudHex($event.target.value)" />
              <i class="fas fa-plus" aria-hidden="true"></i>
            </label>
          </div>
        </div>

        <div class="pers__color-tools">
          <div class="pers__wheel-wrap">
            <canvas ref="wheel" width="168" height="168" class="pers__wheel" tabindex="0" role="slider" :aria-valuenow="Math.round(wheel.h)" aria-valuemin="0" aria-valuemax="359" :aria-label="$t('#L_GS_P_WHEEL')" @pointerdown="onWheelDown" @pointermove="onWheelMove" @pointerup="onWheelUp" @pointercancel="onWheelUp" @keydown="onWheelKey"></canvas>
            <span class="pers__wheel-cursor" :style="wheelCursorStyle" aria-hidden="true"></span>
          </div>
          <div class="pers__color-fields">
            <label class="pers__field pers__field--hex"><span>HEX</span><input type="text" class="pers__input pers__input--mono" :value="hudHex" spellcheck="false" autocomplete="off" maxlength="7" :aria-label="$t('#L_GS_P_HEX')" @change="onHudHex($event.target.value)" /></label>
            <div class="pers__rgb">
              <label v-for="ch in ['r','g','b']" :key="ch" class="pers__field"><span>{{ ch.toUpperCase() }}</span><input type="number" class="pers__input" min="0" max="255" inputmode="numeric" autocomplete="off" :name="'hud-' + ch" :value="value.hudColor[ch]" @change="onHudChannel(ch, $event.target.value)" /></label>
            </div>
          </div>
        </div>

        <label class="pers__slider">
          <span><b>{{ $t("#L_GS_P_ALPHA") }}</b><output>{{ hudAlphaLabel }}</output></span>
          <input type="range" min="-1" max="255" step="1" :value="value.hudAlpha" :aria-label="$t('#L_GS_P_ALPHA')" @input="set('hudAlpha', Number($event.target.value))" />
        </label>
      </div>

      <div v-show="activePanel === 'cross'" class="pers__panel" role="tabpanel">
        <div class="pers__feature-head">
          <div>
            <strong>{{ $t("#L_GS_P_CROSS_TITLE") }}</strong>
            <span>{{ $t("#L_GS_P_CROSS_HINT") }}</span>
          </div>
          <label class="settings-switch">
            <input type="checkbox" :checked="value.crossEnabled" :aria-label="$t('#L_GS_P_CROSS_TITLE')" @change="toggleCrossEnabled($event.target.checked)" />
            <span class="settings-switch__track"><span class="settings-switch__thumb"></span></span>
          </label>
        </div>

        <div class="pers__group">
          <span class="pers__label">{{ $t("#L_GS_P_PRESETS") }}</span>
          <div class="pers__presets">
            <button v-for="p in crossPresets" :key="p.id" type="button" class="pers__preset" :class="{ active: p.id === 'standard' ? !value.crossEnabled : value.crossEnabled && value.crossPreset === p.id }" @click="applyCrossPreset(p.id)">
              <span>{{ $t(p.title) }}</span>
            </button>
          </div>
        </div>

        <div class="pers__cross-options">
          <label v-if="!goldsrc" class="pers__option">
            <span><b>{{ $t("#L_GS_P_HIDE_QUICKINFO") }}</b><small>{{ $t("#L_GS_P_HIDE_QUICKINFO_HINT") }}</small></span>
            <input type="checkbox" :checked="value.hideQuickInfo" @change="set('hideQuickInfo', $event.target.checked)" />
          </label>
          <label class="pers__option">
            <span><b>{{ $t("#L_GS_P_AFFECTS_CROSS") }}</b><small>{{ $t("#L_GS_P_AFFECTS_CROSS_HINT") }}</small></span>
            <input type="checkbox" :checked="value.hudAffectsCrosshair" @change="set('hudAffectsCrosshair', $event.target.checked)" />
          </label>
        </div>

        <div v-show="value.crossEnabled" class="pers__cross-body">
          <div v-show="!value.hudAffectsCrosshair" class="pers__group">
            <span class="pers__label">{{ $t("#L_GS_P_CROSS_COLOR") }}</span>
            <div class="pers__swatches">
              <button v-for="p in crossColorPresets" :key="p.id" type="button" class="pers__swatch" :class="{ active: isCrossColor(p) }" :style="{ background: p.css }" :title="p.label" :aria-label="p.label" @click="setCrossColor(p)"></button>
              <label class="pers__native" :title="$t('#L_GS_P_PICK')"><input type="color" :value="crossHex" :aria-label="$t('#L_GS_P_PICK')" @input="onCrossHex($event.target.value)" /><i class="fas fa-plus" aria-hidden="true"></i></label>
              <code class="pers__cross-hex">{{ crossHex.toUpperCase() }}</code>
            </div>
          </div>

          <div class="pers__slider-grid">
            <label class="pers__slider"><span><b>{{ $t("#L_GS_P_SIZE") }}</b><output>{{ value.crossSize }}</output></span><input type="range" :aria-label="$t('#L_GS_P_SIZE')" min="0" max="40" :value="value.crossSize" @input="set('crossSize', Number($event.target.value))" /></label>
            <label class="pers__slider"><span><b>{{ $t("#L_GS_P_GAP") }}</b><output>{{ value.crossGap }}</output></span><input type="range" :aria-label="$t('#L_GS_P_GAP')" :min="goldsrc ? -30 : 0" max="30" :value="value.crossGap" @input="set('crossGap', Number($event.target.value))" /></label>
            <label class="pers__slider"><span><b>{{ $t("#L_GS_P_THICK") }}</b><output>{{ value.crossThickness }}</output></span><input type="range" :aria-label="$t('#L_GS_P_THICK')" min="1" max="10" :value="value.crossThickness" @input="set('crossThickness', Number($event.target.value))" /></label>
            <label class="pers__slider"><span><b>{{ $t("#L_GS_P_ALPHA_C") }}</b><output>{{ value.crossAlpha }}</output></span><input type="range" :aria-label="$t('#L_GS_P_ALPHA_C')" min="0" max="255" :value="value.crossAlpha" @input="set('crossAlpha', Number($event.target.value))" /></label>
          </div>

          <button type="button" class="pers__advanced-toggle" :aria-expanded="showAdvanced ? 'true' : 'false'" @click="showAdvanced = !showAdvanced">
            {{ $t("#L_GS_P_ADVANCED") }}<i class="fas fa-chevron-down" :class="{ open: showAdvanced }" aria-hidden="true"></i>
          </button>
          <div v-show="showAdvanced" class="pers__advanced">
            <div class="pers__slider-grid">
              <label class="pers__slider"><span><b>{{ $t("#L_GS_P_DOT") }}</b><output>{{ value.crossDotSize }}</output></span><input type="range" :aria-label="$t('#L_GS_P_DOT')" min="0" max="12" :value="value.crossDotSize" @input="set('crossDotSize', Number($event.target.value))" /></label>
              <label class="pers__slider"><span><b>{{ $t("#L_GS_P_CIRCLE") }}</b><output>{{ value.crossCircleRadius }}</output></span><input type="range" :aria-label="$t('#L_GS_P_CIRCLE')" min="0" max="40" :value="value.crossCircleRadius" @input="set('crossCircleRadius', Number($event.target.value))" /></label>
            </div>
            <div class="pers__dot-color">
              <label class="pers__check"><input type="checkbox" :checked="!!value.crossDotColor" @change="set('crossDotColor', $event.target.checked ? hexToColor(crossHex) : null)" />{{ $t("#L_GS_P_DOT_COLOR") }}</label>
              <input v-if="value.crossDotColor" type="color" :value="dotHex" :aria-label="$t('#L_GS_P_DOT_COLOR')" @input="set('crossDotColor', hexToColor($event.target.value))" />
            </div>
            <label v-if="value.crossOutline" class="pers__slider"><span><b>{{ $t("#L_GS_P_OUTLINE_THICK") }}</b><output>{{ value.crossOutlineThickness }}</output></span><input type="range" :aria-label="$t('#L_GS_P_OUTLINE_THICK')" min="1" max="4" :value="value.crossOutlineThickness" @input="set('crossOutlineThickness', Number($event.target.value))" /></label>
            <div class="pers__toggles">
              <label class="pers__check"><input type="checkbox" :checked="value.crossOutline" @change="set('crossOutline', $event.target.checked)" />{{ $t("#L_GS_P_OUTLINE") }}</label>
              <label class="pers__check"><input type="checkbox" :checked="value.crossHideDefault" @change="set('crossHideDefault', $event.target.checked)" />{{ $t("#L_GS_P_HIDE_DEF") }}</label>
            </div>
            <div class="pers__directions">
              <span>{{ $t("#L_GS_P_LINES") }}</span>
              <label><input type="checkbox" :checked="value.crossTopLine" @change="set('crossTopLine', $event.target.checked)" />↑</label>
              <label><input type="checkbox" :checked="value.crossRightLine" @change="set('crossRightLine', $event.target.checked)" />→</label>
              <label><input type="checkbox" :checked="value.crossBottomLine" @change="set('crossBottomLine', $event.target.checked)" />↓</label>
              <label><input type="checkbox" :checked="value.crossLeftLine" @change="set('crossLeftLine', $event.target.checked)" />←</label>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<script>
const HUD_PRESET_LIST = [
  { id: "stock", r: 255, g: 220, b: 0, css: "#ffdC00" },
  { id: "white", r: 255, g: 255, b: 255, css: "#ffffff" },
  { id: "green", r: 0, g: 255, b: 0, css: "#00ff00" },
  { id: "cyan", r: 0, g: 255, b: 255, css: "#00ffff" },
  { id: "blue", r: 80, g: 160, b: 255, css: "#50a0ff" },
  { id: "orange", r: 255, g: 128, b: 0, css: "#ff8000" },
  { id: "pink", r: 255, g: 80, b: 180, css: "#ff50b4" },
  { id: "purple", r: 180, g: 80, b: 255, css: "#b450ff" },
];

const CROSS_PRESET_IDS = [
  { id: "standard", title: "#L_GS_P_PR_STD" },
  { id: "dot", title: "#L_GS_P_PR_DOT" },
  { id: "classic", title: "#L_GS_P_PR_CLASSIC" },
  { id: "circle", title: "#L_GS_P_PR_CIRCLE" },
  { id: "crossdot", title: "#L_GS_P_PR_CROSSDOT" },
];

const CROSS_PRESET_VALUES = {
  dot: { crossColor: { r: 0, g: 255, b: 0 }, crossAlpha: 255, crossThickness: 2, crossSize: 0, crossGap: 0, crossOutline: true, crossOutlineThickness: 1, crossCircleRadius: 0, crossDotColor: null, crossDotSize: 4, crossTopLine: false, crossBottomLine: false, crossLeftLine: false, crossRightLine: false, crossHideDefault: true },
  classic: { crossColor: { r: 0, g: 255, b: 0 }, crossAlpha: 255, crossThickness: 2, crossSize: 10, crossGap: 3, crossOutline: false, crossOutlineThickness: 1, crossCircleRadius: 0, crossDotColor: null, crossDotSize: 0, crossTopLine: true, crossBottomLine: true, crossLeftLine: true, crossRightLine: true, crossHideDefault: true },
  circle: { crossColor: { r: 0, g: 255, b: 0 }, crossAlpha: 255, crossThickness: 2, crossSize: 0, crossGap: 0, crossOutline: true, crossOutlineThickness: 1, crossCircleRadius: 12, crossDotColor: null, crossDotSize: 2, crossTopLine: false, crossBottomLine: false, crossLeftLine: false, crossRightLine: false, crossHideDefault: true },
  crossdot: { crossColor: { r: 255, g: 255, b: 0 }, crossAlpha: 255, crossThickness: 2, crossSize: 8, crossGap: 4, crossOutline: true, crossOutlineThickness: 1, crossCircleRadius: 0, crossDotColor: null, crossDotSize: 2, crossTopLine: true, crossBottomLine: true, crossLeftLine: true, crossRightLine: true, crossHideDefault: true },
};

function clamp(v, a, b) {
  v = Math.round(Number(v));
  if (!isFinite(v)) return a;
  return Math.max(a, Math.min(b, v));
}
function toHex(n) {
  return clamp(n, 0, 255).toString(16).padStart(2, "0");
}
function rgbToHex(c) {
  return `#${toHex(c.r)}${toHex(c.g)}${toHex(c.b)}`;
}
function hexToRgb(hex, fallback) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(hex || "").trim());
  if (!m) return null;
  const v = m[1];
  return {
    r: parseInt(v.slice(0, 2), 16),
    g: parseInt(v.slice(2, 4), 16),
    b: parseInt(v.slice(4, 6), 16),
  };
}
function rgbToHsv(c) {
  const r = c.r / 255, g = c.g / 255, b = c.b / 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  const d = max - min;
  let h = 0;
  if (d !== 0) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  const s = max === 0 ? 0 : d / max;
  return { h, s, v: max };
}
function hsvToRgb(h, s, v) {
  const c = v * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  let r1 = 0, g1 = 0, b1 = 0;
  if (h < 60) { r1 = c; g1 = x; }
  else if (h < 120) { r1 = x; g1 = c; }
  else if (h < 180) { g1 = c; b1 = x; }
  else if (h < 240) { g1 = x; b1 = c; }
  else if (h < 300) { r1 = x; b1 = c; }
  else { r1 = c; b1 = x; }
  const m = v - c;
  return {
    r: Math.round((r1 + m) * 255),
    g: Math.round((g1 + m) * 255),
    b: Math.round((b1 + m) * 255),
  };
}
export default {
  name: "GhostingPersonalization",
  emits: ["input", "panel"],
  props: {
    value: { type: Object, required: true },
    // HL1 through Bunnymod XT: no quick-info arcs, and HUD alpha 0 means the
    // game's own alpha rather than invisible.
    goldsrc: { type: Boolean, default: false },
  },
  data() {
    const hsv = rgbToHsv(this.value.hudColor || { r: 255, g: 220, b: 0 });
    const swatches = HUD_PRESET_LIST.map((p) => Object.assign({}, p, { label: `RGB ${p.r} ${p.g} ${p.b}` }));
    return {
      hudPresets: swatches,
      crossPresets: CROSS_PRESET_IDS,
      crossColorPresets: swatches,
      activePanel: "hud",
      showAdvanced: false,
      wheel: { h: hsv.h, s: Math.max(0.05, hsv.s) },
      wheelDrag: false,
    };
  },
  computed: {
    hudHex() {
      return rgbToHex(this.value.hudColor);
    },
    crossHex() {
      return rgbToHex(
        this.value.hudAffectsCrosshair ? this.value.hudColor : this.value.crossColor
      );
    },
    dotHex() {
      return this.value.crossDotColor ? rgbToHex(this.value.crossDotColor) : this.crossHex;
    },
    wheelCursorStyle() {
      const rad = (this.wheel.h * Math.PI) / 180;
      return {
        left: `${50 + 42 * this.wheel.s * Math.cos(rad)}%`,
        top: `${50 + 42 * this.wheel.s * Math.sin(rad)}%`,
      };
    },
    hudAlphaLabel() {
      const auto = this.goldsrc ? this.value.hudAlpha <= 0 : this.value.hudAlpha === -1;
      return auto ? this.$t("#L_GS_P_AUTO") : String(this.value.hudAlpha);
    },
  },
  watch: {
    // The preview next to the editor shows the HUD or the crosshair to match.
    activePanel: {
      immediate: true,
      handler(panel) {
        this.$emit("panel", panel);
      },
    },
    value: {
      deep: true,
      handler() {
        const hsv = rgbToHsv(this.value.hudColor);
        // Не дёргаем курсор пока тянут колесо.
        if (!this.wheelDrag) this.wheel = { h: hsv.h, s: Math.max(0.05, hsv.s) };
      },
    },
  },
  methods: {
    hexToColor(hex) {
      return hexToRgb(hex);
    },
    emit(patch) {
      this.$emit("input", Object.assign({}, this.value, patch));
    },
    set(key, val) {
      const patch = { [key]: val };
      if (key.startsWith("cross") && key !== "crossEnabled" && this.value.crossEnabled) {
        patch.crossPreset = "custom";
      }
      this.emit(patch);
    },
    isHudPreset(p) {
      const c = this.value.hudColor;
      return c.r === p.r && c.g === p.g && c.b === p.b;
    },
    setHudPreset(p) {
      this.emit({ hudColor: { r: p.r, g: p.g, b: p.b }, hudEnabled: true });
    },
    onHudHex(hex) {
      const c = hexToRgb(hex);
      if (c) this.emit({ hudColor: c, hudEnabled: true });
    },
    onHudChannel(ch, val) {
      const c = Object.assign({}, this.value.hudColor, { [ch]: clamp(val, 0, 255) });
      this.emit({ hudColor: c, hudEnabled: true });
    },
    isCrossColor(p) {
      const c = this.value.crossColor;
      return c.r === p.r && c.g === p.g && c.b === p.b;
    },
    setCrossColor(p) {
      this.emit({ crossColor: { r: p.r, g: p.g, b: p.b }, crossPreset: "custom", crossEnabled: true });
    },
    onCrossHex(hex) {
      const c = hexToRgb(hex);
      if (c) this.emit({ crossColor: c, crossPreset: "custom", crossEnabled: true });
    },
    applyCrossPreset(id) {
      if (id === "standard") {
        this.emit({ crossPreset: "standard", crossEnabled: false });
        return;
      }
      const v = CROSS_PRESET_VALUES[id];
      if (!v) return;
      this.emit(Object.assign({ crossPreset: id, crossEnabled: true }, JSON.parse(JSON.stringify(v))));
    },
    toggleCrossEnabled(enabled) {
      if (enabled && this.value.crossPreset === "standard") {
        this.applyCrossPreset("classic");
        return;
      }
      this.set("crossEnabled", enabled);
    },
    // --- wheel ---
    drawWheel() {
      const canvas = this.$refs.wheel;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      const size = canvas.width, center = size / 2, radius = center - 4;
      const img = ctx.createImageData(size, size);
      for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
          const dx = x - center, dy = y - center;
          const dist = Math.hypot(dx, dy);
          if (dist > radius) continue;
          const hue = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
          const c = hsvToRgb(hue, Math.min(1, dist / radius), 1);
          const i = (y * size + x) * 4;
          img.data[i] = c.r; img.data[i + 1] = c.g; img.data[i + 2] = c.b;
          img.data[i + 3] = Math.min(255, (radius - dist) * 40);
        }
      }
      ctx.putImageData(img, 0, 0);
    },
    wheelToColor(clientX, clientY) {
      const canvas = this.$refs.wheel;
      const rect = canvas.getBoundingClientRect();
      const dx = clientX - rect.left - rect.width / 2;
      const dy = clientY - rect.top - rect.height / 2;
      const h = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
      const s = Math.min(1, Math.hypot(dx, dy) / (rect.width * 0.48));
      this.wheel = { h, s: Math.max(0.02, s) };
      const v = Math.max(rgbToHsv(this.value.hudColor).v, 0.7);
      this.emit({ hudColor: hsvToRgb(h, Math.max(0.02, s), v), hudEnabled: true });
    },
    onWheelDown(e) {
      this.wheelDrag = true;
      if (e.target.setPointerCapture && e.pointerId !== undefined) {
        try { e.target.setPointerCapture(e.pointerId); } catch (ignored) { /* noop */ }
      }
      this.wheelToColor(e.clientX, e.clientY);
    },
    onWheelMove(e) {
      if (this.wheelDrag) this.wheelToColor(e.clientX, e.clientY);
    },
    onWheelUp() {
      this.wheelDrag = false;
    },
    onWheelKey(e) {
      let h = this.wheel.h, s = this.wheel.s, used = true;
      if (e.key === "ArrowLeft") h = (h - 3 + 360) % 360;
      else if (e.key === "ArrowRight") h = (h + 3) % 360;
      else if (e.key === "ArrowUp") s = Math.min(1, s + 0.03);
      else if (e.key === "ArrowDown") s = Math.max(0, s - 0.03);
      else used = false;
      if (used) {
        e.preventDefault();
        this.wheel = { h, s: Math.max(0.02, s) };
        const v = Math.max(rgbToHsv(this.value.hudColor).v, 0.7);
        this.emit({ hudColor: hsvToRgb(h, Math.max(0.02, s), v), hudEnabled: true });
      }
    },
  },
  mounted() {
    this.drawWheel();
  },
};
</script>

<style lang="scss" scoped>
/* One section of the mod settings: it takes the settings column, and the
   preview next to it belongs to the mod settings. */
.pers {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: var(--l-modal-bar);
  color: var(--l-text);
  color-scheme: dark;

  &__tabs {
    display: grid;
    flex: none;
    grid-template-columns: 1fr 1fr;
    min-height: 44px;
    border-bottom: 1px solid var(--l-border);

    button {
      position: relative;
      display: flex;
      align-items: center;
      justify-content: center;
      gap: 8px;
      border: 0;
      background: transparent;
      color: var(--l-text-dim);
      font: inherit;
      font-size: 13px;
      font-weight: 500;
      cursor: pointer;
      transition: color var(--l-fast), background var(--l-fast);
    }
    button + button { border-left: 1px solid var(--l-border); }
    button:hover { background: rgba(255, 255, 255, 0.04); color: var(--l-text); }
    button.active { background: var(--accent-soft); color: var(--l-text); }
    button.active::after { content: ""; position: absolute; right: 16px; bottom: -1px; left: 16px; height: 2px; background: var(--accent); }
    button:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
    i { color: var(--accent); font-size: 12px; }
  }

  &__panel-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  &__panel { padding: 14px; }

  &__feature-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--l-border);

    > div { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
    strong { font-size: 15px; font-weight: 600; line-height: 1.3; }
    span { color: var(--l-text-faint); font-size: 12px; line-height: 1.4; text-wrap: pretty; }
  }

  &__group { margin-top: 14px; }
  &__label { display: block; margin-bottom: 8px; color: var(--l-text-faint); font-size: 11.5px; font-weight: 600; letter-spacing: 0.08em; text-transform: uppercase; }

  // Eight colours and the picker fit one row in the narrowest settings column (300 px inside).
  &__swatches { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
  &__swatch,
  &__native {
    position: relative;
    flex: none;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 2px solid rgba(255, 255, 255, 0.14);
    border-radius: 8px;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.2);
    cursor: pointer;
    transition: transform var(--l-fast), border-color var(--l-fast), box-shadow var(--l-fast);
  }
  &__swatch:hover,
  &__native:hover { transform: translateY(-1px); border-color: rgba(255, 255, 255, 0.45); }
  &__swatch:focus-visible,
  &__native:focus-within { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
  &__swatch.active { border-color: #fff; box-shadow: 0 0 0 2px var(--accent), inset 0 0 0 1px rgba(0, 0, 0, 0.25); }
  &__native {
    display: grid;
    place-items: center;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.055);
    color: var(--l-text-dim);

    input { position: absolute; inset: 0; width: 100%; height: 100%; opacity: 0; cursor: pointer; }
    i { font-size: 10px; }
  }

  &__color-tools {
    display: grid;
    grid-template-columns: 108px minmax(0, 1fr);
    align-items: center;
    gap: 14px;
    margin-top: 14px;
    padding: 12px;
    border: 1px solid var(--l-border);
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.022);
  }
  &__wheel-wrap { position: relative; width: 108px; height: 108px; }
  &__wheel { display: block; width: 108px; height: 108px; border-radius: 50%; cursor: crosshair; filter: saturate(0.9); }
  &__wheel:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 3px; }
  &__wheel-cursor { position: absolute; width: 12px; height: 12px; border: 2px solid #fff; border-radius: 50%; box-shadow: 0 1px 7px #000; transform: translate(-50%, -50%); pointer-events: none; }

  &__color-fields { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
  &__rgb { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px; }
  &__field { display: flex; flex-direction: column; gap: 4px; color: var(--l-text-faint); font-size: 11px; font-weight: 600; letter-spacing: 0.06em; }
  &__input {
    box-sizing: border-box;
    width: 100%;
    min-width: 0;
    height: 32px;
    padding: 0 9px;
    border: 1px solid var(--l-border);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font: inherit;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    transition: background var(--l-fast), border-color var(--l-fast);

    &:hover { border-color: var(--l-border-strong); background: rgba(255, 255, 255, 0.07); }
    &:focus { outline: none; border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-soft); }
    &[type="number"] { appearance: textfield; }
    &[type="number"]::-webkit-inner-spin-button,
    &[type="number"]::-webkit-outer-spin-button { margin: 0; appearance: none; }
    &--mono { font-family: Consolas, monospace; text-transform: uppercase; }
  }

  &__slider {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 14px;
    color: var(--l-text-dim);
    font-size: 13px;

    > span { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
    b { color: var(--l-text); font-weight: 500; }
    output { color: var(--accent); font-size: 12px; font-variant-numeric: tabular-nums; font-weight: 600; }
    input { width: 100%; height: 18px; margin: 0; accent-color: var(--accent); cursor: pointer; }
    input:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
  }
  &__slider-grid { display: grid; grid-template-columns: 1fr 1fr; column-gap: 14px; }

  &__option {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 14px;
    padding: 11px 12px;
    border: 1px solid var(--l-border);
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.018);
    cursor: pointer;

    > span { display: flex; flex-direction: column; gap: 3px; }
    b { color: var(--l-text); font-size: 13px; font-weight: 500; }
    small { color: var(--l-text-faint); font-size: 12px; line-height: 1.35; }
    > input { flex: none; width: 18px; height: 18px; accent-color: var(--accent); }
  }
  &__cross-options { display: grid; gap: 8px; margin-top: 12px; }
  &__cross-options &__option { margin-top: 0; }

  &__presets { display: grid; grid-template-columns: repeat(auto-fit, minmax(96px, 1fr)); gap: 6px; }
  &__preset {
    min-width: 0;
    min-height: 34px;
    padding: 6px 8px;
    overflow: hidden;
    border: 1px solid var(--l-border);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.04);
    color: var(--l-text-dim);
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
    transition: border-color var(--l-fast), background var(--l-fast), color var(--l-fast);

    &:hover { border-color: var(--l-border-strong); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
    &.active { border-color: var(--accent-border); background: var(--accent-soft); color: var(--l-text); }
  }

  &__cross-body { margin-top: 2px; }
  &__cross-hex { margin-left: auto; color: var(--l-text-dim); font-family: Consolas, monospace; font-size: 12.5px; }

  &__advanced-toggle {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    margin-top: 14px;
    padding: 10px 0;
    border: 0;
    border-top: 1px solid var(--l-border);
    border-bottom: 1px solid var(--l-border);
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;

    &:hover { color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
    i { color: var(--l-text-faint); font-size: 10px; transition: transform var(--l-fast); }
    i.open { transform: rotate(180deg); }
  }
  &__advanced { padding-top: 1px; }

  &__dot-color {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 14px;

    input[type="color"] { width: 40px; height: 28px; padding: 2px; border: 1px solid var(--l-border); border-radius: 6px; background: transparent; cursor: pointer; }
  }
  &__toggles { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 12px; }
  &__check {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--l-text-dim);
    font-size: 13px;
    cursor: pointer;

    input { accent-color: var(--accent); }
  }
  &__directions {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 12px;
    color: var(--l-text-dim);
    font-size: 13px;

    > span { margin-right: auto; }
    label { position: relative; display: grid; place-items: center; width: 32px; height: 28px; border: 1px solid var(--l-border); border-radius: 6px; color: var(--l-text-dim); cursor: pointer; }
    input { position: absolute; opacity: 0; }
    label:has(input:checked) { border-color: var(--accent-border); background: var(--accent-soft); color: var(--l-text); }
    label:has(input:focus-visible) { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
  }
}

.settings-switch {
  position: relative;
  display: inline-flex;
  flex: none;
  cursor: pointer;

  input { position: absolute; inset: 0; width: 100%; height: 100%; margin: 0; opacity: 0; cursor: pointer; }
  &__track { display: flex; align-items: center; box-sizing: border-box; width: 42px; height: 24px; padding: 3px; border-radius: 12px; background: rgba(255, 255, 255, 0.12); transition: background var(--l-med); }
  &__thumb { width: 18px; height: 18px; border-radius: 50%; background: #fff; transition: transform var(--l-med); }
  input:checked + &__track { background: var(--accent); }
  input:checked + &__track &__thumb { background: var(--l-switch-thumb-on, #fff); transform: translateX(18px); }
  input:focus-visible + &__track { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
}

/* The smallest launcher window with the largest interface scale. */
@media (max-height: 640px) {
  .pers__panel { padding: 12px; }
  .pers__group, .pers__slider, .pers__option, .pers__color-tools { margin-top: 10px; }
}

@media (prefers-reduced-motion: reduce) {
  .pers *, .settings-switch * { transition: none !important; }
}
</style>
