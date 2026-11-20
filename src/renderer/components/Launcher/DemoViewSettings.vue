<template>
  <!-- «Дополнительные настройки» предпросмотра и экспорта: ссылка раскрывает
       переключатели вида в кадре. Чего игра не умеет — переключатель неактивен,
       с пояснением (возможности адаптера: demo-inspect render.view, demo-options capabilities). -->
  <div class="dvs" @keydown.esc="onEsc">
    <button ref="toggle" type="button" class="dvs__more" :aria-expanded="open ? 'true' : 'false'" @click="toggle">
      {{ $t("#L_DEMO_MORE") }}<i class="fas" :class="open ? 'fa-chevron-up' : 'fa-chevron-down'" aria-hidden="true"></i>
    </button>
    <!-- В оверлее плеера (popover) панель — поверх окна: внутри оверлея высотой
         ~180 px на Full HD она раскрывалась ниже видимой части. -->
    <Teleport to=".launcher-root" :disabled="!popover">
    <div v-if="open" ref="panel" class="dvs__panel" :class="{ 'is-popover': popover }" :style="popover ? place : null" role="group"
      :aria-label="$t('#L_DEMO_MORE')" @keydown.esc="onEsc">
      <label v-for="row in rows" :key="row.key" class="dvs__row" :class="{ 'is-off': !row.available }">
        <span class="gx-switch">
          <input type="checkbox" :checked="row.checked" :disabled="!row.available" :aria-label="row.label" @change="set(row.key, $event.target.checked)" />
          <span class="gx-switch__track"><span class="gx-switch__thumb"></span></span>
        </span>
        <span class="dvs__text">
          <span class="dvs__label">{{ row.label }}</span>
          <span class="dvs__hint">{{ $t(row.hint) }}</span>
        </span>
      </label>
    </div>
    </Teleport>
  </div>
</template>

<script>
// Что недоступно игре, показывается в том виде, каким будет в кадре (как у игрока).
const FIXED = { keyhud: false, hud: true, ownConfig: true, ownHud: true };

export default {
  name: "DemoViewSettings",
  emits: ["update:modelValue"],
  props: {
    // { keyhud, hud, ownConfig, ownHud } — выбор игрока.
    modelValue: { type: Object, required: true },
    // Те же ключи: что умеет адаптер игры.
    caps: { type: Object, default: () => ({}) },
    // hl1 — свои пояснения (BXT, спрайты паков HUD).
    game: { type: String, default: "" },
    // Панель всплывает поверх окна у ссылки, а не раскрывается в потоке.
    popover: { type: Boolean, default: false },
  },
  data() {
    return { open: false, place: {} };
  },
  computed: {
    rows() {
      const hl1 = this.game === "hl1";
      const row = (key, label, hint, missing) => {
        const available = !!this.caps[key];
        return { key, label, hint: available ? hint : missing, available, checked: available ? this.modelValue[key] === true : FIXED[key] };
      };
      return [
        row("keyhud", "KeyHUD", "#L_DEMO_EX_KEYHUD", "#L_DEMO_EX_KEYHUD_NA"),
        row("hud", this.$t("#L_DEMO_VIEW_HUD"), "#L_DEMO_VIEW_HUD_HINT", "#L_DEMO_VIEW_HUD_NA"),
        row("ownConfig", this.$t("#L_DEMO_VIEW_CONFIG"), "#L_DEMO_VIEW_CONFIG_HINT", "#L_DEMO_VIEW_CONFIG_NA"),
        row("ownHud", this.$t("#L_DEMO_VIEW_OWN_HUD"), hl1 ? "#L_DEMO_VIEW_OWN_HUD_HL1" : "#L_DEMO_VIEW_OWN_HUD_HINT", "#L_DEMO_VIEW_OWN_HUD_NA"),
      ];
    },
  },
  watch: {
    open(value) {
      const method = value ? "addEventListener" : "removeEventListener";
      document[method]("pointerdown", this.onOutside, true);
      window[method]("resize", this.placePanel);
      window[method]("scroll", this.placePanel, true);
    },
  },
  // Страница «Демо» в keep-alive: перенесённую панель уход со страницы не уберёт.
  deactivated() {
    this.open = false;
  },
  beforeUnmount() {
    this.open = false;
    document.removeEventListener("pointerdown", this.onOutside, true);
    window.removeEventListener("resize", this.placePanel);
    window.removeEventListener("scroll", this.placePanel, true);
  },
  methods: {
    // Раскрытая панель сразу на виду: в потоке — докрутка до неё, поверх окна — место у ссылки.
    toggle() {
      this.open = !this.open;
      if (!this.open) return;
      this.$nextTick(() => {
        const panel = this.$refs.panel;
        if (!panel) return;
        if (!this.popover) return panel.scrollIntoView({ block: "nearest" });
        this.placePanel();
        // Панель в конце окна: Tab от ссылки до переключателей не дошёл бы.
        const first = panel.querySelector("input:not(:disabled)");
        if (first) first.focus({ preventScroll: true });
      });
    },
    // Под ссылкой, если там хватает места; иначе туда, где места больше, с прокруткой.
    placePanel() {
      const toggle = this.$refs.toggle;
      const panel = this.$refs.panel;
      if (!this.popover || !this.open || !toggle || !panel) return;
      const margin = 12;
      const gap = 6;
      const rect = toggle.getBoundingClientRect();
      const below = window.innerHeight - rect.bottom - gap - margin;
      const above = rect.top - gap - margin;
      const width = panel.offsetWidth;
      const left = Math.max(margin, Math.min(window.innerWidth - margin - width, rect.left + rect.width / 2 - width / 2));
      this.place =
        panel.scrollHeight <= below || below >= above
          ? { left: left + "px", top: rect.bottom + gap + "px", maxHeight: below + "px" }
          : { left: left + "px", bottom: window.innerHeight - rect.top + gap + "px", maxHeight: above + "px" };
    },
    onOutside(event) {
      const inside = (ref) => this.$refs[ref] && this.$refs[ref].contains(event.target);
      if (this.popover && !inside("panel") && !inside("toggle")) this.open = false;
    },
    onEsc(event) {
      if (!this.open) return;
      event.preventDefault();
      event.stopPropagation();
      this.open = false;
      if (this.$refs.toggle) this.$refs.toggle.focus();
    },
    set(key, value) {
      this.$emit("update:modelValue", Object.assign({}, this.modelValue, { [key]: value }));
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

.dvs {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  width: 100%;

  /* Ссылка — голубая в любой теме, и в «Бумаге» (просьба владельца): на тёмном фоне читается везде. */
  &__more {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: #4db2ff;
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    text-decoration: underline;
    text-underline-offset: 3px;
    cursor: pointer;

    i { font-size: 10px; text-decoration: none; }
    &:hover { color: #7cc6ff; }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
  }

  &__panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: min(100%, 440px);
    padding: 12px 14px;
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-bg);
    box-shadow: inset 0 0 0 1px var(--l-modal-border);
    text-align: left;

    &.is-popover {
      @include gx-scrollbar;
      position: fixed;
      z-index: 1000;
      width: min(440px, calc(100vw - 24px));
      overflow-y: auto;
      overscroll-behavior: contain;
      box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
    }
  }

  &__row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    cursor: pointer;

    .gx-switch { margin-top: 1px; }
    &.is-off { cursor: default; }
    &.is-off .gx-switch { opacity: 0.45; }
  }

  &__text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  &__label {
    color: var(--l-text);
    font-size: 13px;
    font-weight: 600;
    line-height: 18px;
  }

  &__hint {
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 1.4;
  }
}
</style>
