<template>
  <!-- Полоса между двумя панелями: тянут мышью, стрелками по 16 px, двойной щелчок или Enter — как было.
       Размер (v-model, px CSS) применяет родитель; null — его размер по умолчанию. -->
  <div
    class="pane-split"
    :class="['pane-split--' + orientation, { 'is-dragging': dragging }]"
    role="separator"
    tabindex="0"
    :aria-orientation="orientation"
    :aria-valuenow="now"
    :aria-valuemin="min"
    :aria-valuemax="max"
    :aria-label="$t(label)"
    :title="$t('#L_SPLIT_HINT')"
    @pointerdown="down"
    @pointermove="move"
    @pointerup="up"
    @pointercancel="up"
    @lostpointercapture="up"
    @dblclick="reset"
    @keydown="key"
    @focus="measure"
  >
    <span class="pane-split__grip" aria-hidden="true"></span>
  </div>
</template>

<script>
const { clampSize, readSize, writeSize, keySize } = require("./pane-size");

function storage() {
  try {
    return window.localStorage;
  } catch (error) {
    return null;
  }
}

export default {
  name: "PaneSplitter",
  emits: ["update:modelValue"],
  props: {
    modelValue: { type: Number, default: null },
    // Под каким именем запомнить размер: место и игра.
    storageKey: { type: String, required: true },
    // horizontal — полоса между верхом и низом, vertical — между колонками (как aria-orientation).
    orientation: { type: String, default: "vertical" },
    min: { type: Number, required: true },
    // Сколько оставить панели после полосы (её минимум в CSS родителя).
    rest: { type: Number, default: 0 },
    // Чей размер меняется; по умолчанию — панель перед полосой.
    target: { type: Function, default: null },
    // Ключ локали для aria-label.
    label: { type: String, required: true },
  },
  data() {
    return { dragging: false, now: null, max: null };
  },
  computed: {
    vertical() {
      return this.orientation === "vertical";
    },
  },
  watch: {
    storageKey: {
      immediate: true,
      handler(key) {
        this.$emit("update:modelValue", readSize(storage(), key));
        // Другая игра — другой размер: aria-valuenow обновится и без смены окна.
        if (this.$el) this.$nextTick(this.measure);
      },
    },
  },
  mounted() {
    this.measure();
    // Панель меняется и без смены окна (вся клавиатура или основная) — aria-valuenow следом.
    this.observer = new ResizeObserver(() => this.measure());
    this.observer.observe(this.$el.parentElement);
    if (this.target) this.observer.observe(this.target());
  },
  beforeUnmount() {
    if (this.observer) this.observer.disconnect();
  },
  methods: {
    side(el) {
      const box = el.getBoundingClientRect();
      return this.vertical ? box.width : box.height;
    },
    // Сколько панель может вырасти — столько, сколько отдаст соседняя до своего минимума
    // (уже меньше него — расти некуда, но и сама не сожмётся). getBoundingClientRect
    // и clientX/Y — в одних px CSS и при zoomFactor окна.
    measure() {
      const pane = this.target ? this.target() : this.$el.previousElementSibling;
      const next = this.$el.nextElementSibling;
      if (!pane || !next) return null;
      const size = this.side(pane);
      this.now = Math.round(size);
      this.max = Math.max(this.min, this.now, Math.round(size + this.side(next) - this.rest));
      return size;
    },
    coord(event) {
      return this.vertical ? event.clientX : event.clientY;
    },
    down(event) {
      if (event.button !== 0 || this.measure() === null) return;
      event.preventDefault();
      this.$el.setPointerCapture(event.pointerId);
      this.drag = { id: event.pointerId, from: this.coord(event), size: this.now, value: this.modelValue, moved: false };
      this.dragging = true;
    },
    move(event) {
      const drag = this.drag;
      if (!drag || event.pointerId !== drag.id) return;
      const delta = this.coord(event) - drag.from;
      if (!delta && !drag.moved) return;
      drag.moved = true;
      this.now = clampSize(drag.size + delta, this.min, this.max);
      this.$emit("update:modelValue", this.now);
    },
    up(event) {
      const drag = this.drag;
      if (!drag || event.pointerId !== drag.id) return;
      this.drag = null;
      this.dragging = false;
      // Щелчок без движения (и первый щелчок двойного) размер не закрепляет.
      if (drag.moved) this.settle(drag);
    },
    key(event) {
      if (event.key === "Enter") {
        event.preventDefault();
        this.reset();
        return;
      }
      if (this.measure() === null) return;
      const next = keySize(this.now, event.key, this.min, this.max);
      if (next === null) return;
      event.preventDefault();
      const start = { size: this.now, value: this.modelValue };
      this.$emit("update:modelValue", next);
      this.settle(start);
    },
    // CSS родителя может не дать панели вырасти (ширина окна, минимум соседней панели):
    // запоминается размер, который вышел на деле. Не изменился — остаётся прежний, иначе
    // упор в тесном окне закрепил бы размер меньше обычного.
    settle(start) {
      this.$nextTick(() => {
        if (this.measure() === null) return;
        const value = this.now === start.size ? start.value : this.now;
        this.$emit("update:modelValue", value);
        if (value !== start.value) writeSize(storage(), this.storageKey, value);
      });
    },
    reset() {
      this.$emit("update:modelValue", null);
      writeSize(storage(), this.storageKey, null);
      this.$nextTick(this.measure);
    },
  },
};
</script>

<style lang="scss" scoped>
.pane-split {
  position: relative;
  flex: none;
  align-self: stretch;
  display: grid;
  place-items: center;
  touch-action: none;
  user-select: none;
  outline: none;

  &--vertical { width: 12px; cursor: col-resize; }
  &--horizontal { height: 8px; cursor: row-resize; }

  // Тонкая линия во всю длину — только при наведении и перетаскивании.
  &::before {
    content: "";
    position: absolute;
    border-radius: 1px;
    background: var(--accent);
    opacity: 0;
    transition: opacity var(--l-fast);
  }
  &--vertical::before { top: 0; bottom: 0; left: calc(50% - 1px); width: 2px; }
  &--horizontal::before { left: 0; right: 0; top: calc(50% - 1px); height: 2px; }

  // Ручка посередине видна всегда: так понятно, что край тянется.
  &__grip {
    position: relative;
    border-radius: 2px;
    background: var(--l-border-strong);
    transition: background var(--l-fast);
  }
  &--vertical &__grip { width: 4px; height: 36px; }
  &--horizontal &__grip { width: 36px; height: 4px; }

  &:hover::before,
  &.is-dragging::before { opacity: 0.6; }
  &:hover &__grip,
  &.is-dragging &__grip { background: var(--accent); }
  &:focus-visible::before { opacity: 1; background: var(--accent-focus); }
  &:focus-visible &__grip { background: var(--accent-focus); }
}
</style>
