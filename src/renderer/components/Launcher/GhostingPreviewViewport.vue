<template>
  <div class="gv" :class="{ 'gv--framed': framed, 'gv--fill': fill, 'gv--zoomed': zoomed, 'gv--dragging': dragging, 'gv--over-handle': overHandle, 'gv--moving': moving }" @keydown="onKey">
    <div
      ref="stage"
      class="gv__stage"
      role="group"
      tabindex="0"
      :style="stageStyle"
      :aria-label="label"
      :aria-describedby="describedby || null"
      @wheel="onWheel"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="endDrag"
      @pointercancel="endDrag"
      @pointerleave="overHandle = false"
      @dblclick="onDoubleClick"
    >
      <canvas ref="canvas" class="gv__canvas" aria-hidden="true"></canvas>
      <div v-if="message" class="gv__status" role="status">
        <i v-if="loading" class="fas fa-spinner fa-spin" aria-hidden="true"></i>
        <span>{{ message }}</span>
      </div>
    </div>

    <div class="gv__bar" role="toolbar" :aria-label="$t('#L_GS_X_ZOOM_TOOLS')">
      <button type="button" class="gv__btn gv__btn--icon" :disabled="!canZoomOut" :title="$t('#L_GS_X_ZOOM_OUT') + ' (−)'" :aria-label="$t('#L_GS_X_ZOOM_OUT')" @click="zoomBy(1 / STEP)">
        <i class="fas fa-minus" aria-hidden="true"></i>
      </button>
      <output class="gv__level" aria-live="off" :aria-label="$t('#L_GS_X_ZOOM_LEVEL')">{{ percent }}%</output>
      <button type="button" class="gv__btn gv__btn--icon" :disabled="!canZoomIn" :title="$t('#L_GS_X_ZOOM_IN') + ' (+)'" :aria-label="$t('#L_GS_X_ZOOM_IN')" @click="zoomBy(STEP)">
        <i class="fas fa-plus" aria-hidden="true"></i>
      </button>
      <button type="button" class="gv__btn" :aria-pressed="atFit ? 'true' : 'false'" :title="$t('#L_GS_X_ZOOM_FIT_TITLE') + ' (0)'" @click="fitView">{{ $t('#L_GS_X_ZOOM_FIT') }}</button>
      <button type="button" class="gv__btn" :aria-pressed="actual ? 'true' : 'false'" :title="$t('#L_GS_X_ZOOM_ACTUAL') + ' (1)'" @click="actualSize()">100%</button>
      <span class="gv__spacer"></span>
      <button v-if="motion" type="button" class="gv__btn" :aria-pressed="animating ? 'true' : 'false'" @click="$emit('motion')">
        <i class="fas" :class="animating ? 'fa-pause' : 'fa-play'" aria-hidden="true"></i>{{ $t('#L_GS_X_OVERLAY_MOTION') }}
      </button>
      <button v-if="expandable" type="button" class="gv__btn gv__btn--icon" :title="$t('#L_GS_X_PREVIEW_EXPAND')" :aria-label="$t('#L_GS_X_PREVIEW_EXPAND')" @click="$emit('expand')">
        <i class="fas fa-expand" aria-hidden="true"></i>
      </button>
    </div>
  </div>
</template>

<script>
// Zoomable view of a frame that is drawn elsewhere. The frame is kept in
// source pixels; the view stores a zoom relative to "fit" and the source point
// at the centre of the viewport, so resizing never moves what the user looks at.
// Every mouse gesture has a button or key alternative.
const STEP = 1.25;
const MAX_SCALE = 3;

export default {
  name: 'GhostingPreviewViewport',
  emits: ["expand", "motion", "handle-move", "handle-end", "box"],
  props: {
    source: { type: Object, default: null },
    frameWidth: { type: Number, default: 1600 },
    frameHeight: { type: Number, default: 900 },
    // Inside a card: the toolbar is the card's header and the frame touches
    // the card's edges.
    framed: { type: Boolean, default: false },
    // Кадр занимает всю высоту, что ему оставили, а не держит свои пропорции:
    // хозяин подгоняет ширину кадра под размер окна (событие box).
    fill: { type: Boolean, default: false },
    loading: { type: Boolean, default: false },
    message: { type: String, default: '' },
    label: { type: String, default: '' },
    describedby: { type: String, default: '' },
    expandable: { type: Boolean, default: false },
    motion: { type: Boolean, default: false },
    animating: { type: Boolean, default: false },
    // Прямоугольник в пикселях кадра, который можно таскать мышью (оверлей
    // LiveSplit). Нажатие внутри него двигает его, а не кадр: handle-move
    // присылает новый левый верхний угол, handle-end — конец перетаскивания.
    handle: { type: Object, default: null },
  },
  data() {
    return { STEP, zoom: 1, cx: this.frameWidth / 2, cy: this.frameHeight / 2, box: { width: 0, height: 0 }, dragging: false, overHandle: false, moving: false };
  },
  computed: {
    // Screen pixels per source pixel when the whole frame is visible.
    fit() { return this.box.width > 0 && this.box.height > 0 ? Math.min(this.box.width / this.frameWidth, this.box.height / this.frameHeight) : 1; },
    maxZoom() { return Math.max(1, MAX_SCALE / this.fit); },
    scale() { return this.fit * this.zoom; },
    percent() { return Math.round(this.scale * 100); },
    zoomed() { return this.zoom > 1.001; },
    atFit() { return !this.zoomed; },
    actual() { return Math.abs(this.scale - 1) < 0.005; },
    canZoomIn() { return this.zoom < this.maxZoom - 0.001; },
    canZoomOut() { return this.zoomed; },
    // The stage has the frame's proportions, so the frame never sits between bars.
    stageStyle() { return this.fill ? null : { aspectRatio: `${this.frameWidth} / ${this.frameHeight}` }; },
  },
  watch: {
    source() { this.redraw(); },
    // Another screen aspect ratio: the frame is wider or narrower, the view
    // starts over from the whole frame.
    frameWidth(width) { this.zoom = 1; this.cx = width / 2; this.cy = this.frameHeight / 2; this.redraw(); },
    handle() { this.redraw(); },
    overHandle() { this.redraw(); },
  },
  created() {
    this.drag = null;
    this.raf = 0;
    this.observer = null;
  },
  mounted() {
    this.measure();
    if (typeof ResizeObserver === 'function') {
      this.observer = new ResizeObserver(() => this.measure());
      this.observer.observe(this.$refs.stage);
    } else window.addEventListener('resize', this.measure);
  },
  beforeUnmount() {
    cancelAnimationFrame(this.raf);
    if (this.observer) this.observer.disconnect();
    window.removeEventListener('resize', this.measure);
  },
  methods: {
    measure() {
      const stage = this.$refs.stage;
      if (!stage) return;
      const rect = stage.getBoundingClientRect();
      if (rect.width === this.box.width && rect.height === this.box.height) return;
      this.box = { width: rect.width, height: rect.height };
      this.$emit('box', this.box);
      this.clamp();
      this.redraw();
    },
    // Keep the frame covering the viewport; centre it on an axis it does not fill.
    clamp() {
      if (!this.box.width) return;
      const visibleWidth = this.box.width / this.scale, visibleHeight = this.box.height / this.scale;
      this.cx = visibleWidth >= this.frameWidth ? this.frameWidth / 2 : Math.min(this.frameWidth - visibleWidth / 2, Math.max(visibleWidth / 2, this.cx));
      this.cy = visibleHeight >= this.frameHeight ? this.frameHeight / 2 : Math.min(this.frameHeight - visibleHeight / 2, Math.max(visibleHeight / 2, this.cy));
    },
    // The source point under (x, y), in stage pixels, stays under it.
    setScale(next, x, y) {
      if (!this.box.width) return;
      const before = this.scale;
      const px = this.cx + (x - this.box.width / 2) / before, py = this.cy + (y - this.box.height / 2) / before;
      this.zoom = Math.min(this.maxZoom, Math.max(1, next / this.fit));
      const after = this.scale;
      this.cx = px - (x - this.box.width / 2) / after;
      this.cy = py - (y - this.box.height / 2) / after;
      this.clamp();
      this.redraw();
    },
    zoomBy(factor, x = this.box.width / 2, y = this.box.height / 2) { this.setScale(this.scale * factor, x, y); },
    actualSize(x = this.box.width / 2, y = this.box.height / 2) { this.setScale(1, x, y); },
    fitView() {
      this.zoom = 1;
      this.cx = this.frameWidth / 2;
      this.cy = this.frameHeight / 2;
      this.redraw();
    },
    pan(dx, dy) {
      this.cx += dx;
      this.cy += dy;
      this.clamp();
      this.redraw();
    },
    local(event) {
      const rect = this.$refs.stage.getBoundingClientRect();
      return { x: event.clientX - rect.left, y: event.clientY - rect.top };
    },
    onWheel(event) {
      if (!this.box.width) return;
      event.preventDefault();
      const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? this.box.height : 1;
      const point = this.local(event);
      this.zoomBy(Math.exp(-event.deltaY * unit * 0.0018), point.x, point.y);
    },
    // Точка кадра под курсором.
    sourcePoint(event) {
      const point = this.local(event);
      return { x: this.cx + (point.x - this.box.width / 2) / this.scale, y: this.cy + (point.y - this.box.height / 2) / this.scale };
    },
    inHandle(point) {
      const h = this.handle;
      return !!h && point.x >= h.left && point.x <= h.left + h.width && point.y >= h.top && point.y <= h.top + h.height;
    },
    onPointerDown(event) {
      if (event.button !== 0 || !this.box.width) return;
      const point = this.sourcePoint(event);
      if (this.inHandle(point)) {
        this.drag = { id: event.pointerId, handle: true, dx: point.x - this.handle.left, dy: point.y - this.handle.top };
        this.moving = true;
        try { this.$refs.stage.setPointerCapture(event.pointerId); } catch (ignored) { /* the pointer is already gone */ }
        return;
      }
      this.drag = { id: event.pointerId, x: event.clientX, y: event.clientY, cx: this.cx, cy: this.cy };
      this.dragging = true;
      try { this.$refs.stage.setPointerCapture(event.pointerId); } catch (ignored) { /* the pointer is already gone */ }
    },
    onPointerMove(event) {
      if (!this.drag) { if (this.handle && this.box.width) this.overHandle = this.inHandle(this.sourcePoint(event)); return; }
      if (event.pointerId !== this.drag.id) return;
      if (this.drag.handle) {
        const point = this.sourcePoint(event);
        this.$emit('handle-move', { left: point.x - this.drag.dx, top: point.y - this.drag.dy });
        return;
      }
      this.cx = this.drag.cx - (event.clientX - this.drag.x) / this.scale;
      this.cy = this.drag.cy - (event.clientY - this.drag.y) / this.scale;
      this.clamp();
      this.redraw();
    },
    endDrag(event) {
      if (!this.drag || (event && event.pointerId !== this.drag.id)) return;
      const handle = this.drag.handle;
      this.drag = null;
      this.dragging = false;
      this.moving = false;
      if (handle) this.$emit('handle-end');
    },
    onDoubleClick(event) {
      if (this.inHandle(this.sourcePoint(event))) return;
      const point = this.local(event);
      if (this.scale >= 1) this.fitView();
      else this.actualSize(point.x, point.y);
    },
    onKey(event) {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      // Physical codes too: the main row and the numeric pad on any layout.
      const { key, code } = event;
      const step = Math.max(24, (this.box.width / this.scale) * 0.12);
      if (key === '+' || key === '=' || code === 'Equal' || code === 'NumpadAdd') this.zoomBy(STEP);
      else if (key === '-' || key === '_' || code === 'Minus' || code === 'NumpadSubtract') this.zoomBy(1 / STEP);
      else if (key === '0' || code === 'Digit0' || code === 'Numpad0') this.fitView();
      else if (key === '1' || code === 'Digit1' || code === 'Numpad1') this.actualSize();
      else if (key === 'ArrowLeft') this.pan(-step, 0);
      else if (key === 'ArrowRight') this.pan(step, 0);
      else if (key === 'ArrowUp') this.pan(0, -step);
      else if (key === 'ArrowDown') this.pan(0, step);
      else return;
      event.preventDefault();
    },
    // Coalesced repaint; the owner calls paint() directly when it is already
    // inside an animation frame.
    redraw() {
      if (this.raf) return;
      this.raf = requestAnimationFrame(() => { this.raf = 0; this.paint(); });
    },
    paint() {
      const canvas = this.$refs.canvas;
      if (!canvas || !this.source || !this.box.width || !this.box.height) return;
      const ratio = window.devicePixelRatio || 1;
      const width = Math.max(1, Math.round(this.box.width * ratio)), height = Math.max(1, Math.round(this.box.height * ratio));
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      const ctx = canvas.getContext('2d');
      ctx.clearRect(0, 0, width, height);
      ctx.imageSmoothingEnabled = true;
      ctx.imageSmoothingQuality = 'high';
      const scale = this.scale * ratio;
      const left = width / 2 - this.cx * scale, top = height / 2 - this.cy * scale;
      ctx.drawImage(this.source, left, top, this.frameWidth * scale, this.frameHeight * scale);
      // Рамка перетаскиваемого прямоугольника рисуется в пикселях экрана,
      // чтобы оставаться тонкой при любом масштабе.
      const h = this.handle;
      if (h) {
        const x = Math.round(left + h.left * scale) + 0.5, y = Math.round(top + h.top * scale) + 0.5;
        const w = Math.round(h.width * scale) - 1, hh = Math.round(h.height * scale) - 1;
        ctx.save();
        ctx.lineWidth = ratio;
        ctx.setLineDash([5 * ratio, 4 * ratio]);
        ctx.strokeStyle = 'rgba(0, 0, 0, 0.7)';
        ctx.strokeRect(x, y, w, hh);
        ctx.lineDashOffset = 5 * ratio;
        ctx.strokeStyle = this.moving || this.overHandle ? '#fff' : 'rgba(255, 255, 255, 0.75)';
        ctx.strokeRect(x, y, w, hh);
        ctx.restore();
      }
    },
  },
};
</script>

<style lang="scss" scoped>
.gv {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  min-height: 0;
  color: var(--l-text);

  &__stage {
    position: relative;
    min-height: 0;
    overflow: hidden;
    border-radius: var(--l-radius-sm, 10px);
    background: #05090e;
    box-shadow: 0 0 0 1px var(--l-border, rgba(255, 255, 255, 0.1));
    touch-action: none;
    user-select: none;
    overscroll-behavior: contain;
    cursor: default;
  }
  &--zoomed &__stage { cursor: grab; }
  &--dragging &__stage { cursor: grabbing; }
  &--over-handle &__stage { cursor: move; }
  &--moving &__stage { cursor: move; }
  &__stage:focus-visible { outline: 2px solid var(--accent-focus, #4fc3ff); outline-offset: 2px; }

  &__canvas { position: absolute; inset: 0; display: block; width: 100%; height: 100%; }

  &__status {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 16px;
    background: rgba(5, 9, 14, 0.72);
    color: var(--l-text-dim);
    font-size: 13px;
    text-align: center;
    text-wrap: balance;
  }

  &__bar { display: flex; flex: none; align-items: center; gap: 6px; min-height: 30px; }

  &--fill { flex: 1 1 0; }
  &--fill &__stage { flex: 1 1 0; }

  &--framed { gap: 0; }
  &--framed &__bar { order: -1; padding: 7px 8px; border-bottom: 1px solid var(--l-border, rgba(255, 255, 255, 0.1)); }
  &--framed &__stage { border-radius: 0; box-shadow: none; }
  &--framed &__stage:focus-visible { outline-offset: -2px; }
  &__spacer { flex: 1; }
  &__level {
    min-width: 46px;
    color: var(--l-text-dim);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    text-align: center;
  }

  &__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    min-width: 30px;
    height: 30px;
    padding: 0 11px;
    border: 1px solid var(--l-border, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 12px;
    font-weight: 500;
    line-height: 1;
    white-space: nowrap;
    cursor: pointer;
    touch-action: manipulation;
    transition: background var(--l-fast, 0.16s ease), border-color var(--l-fast, 0.16s ease), color var(--l-fast, 0.16s ease);

    &:hover:not(:disabled) { border-color: var(--l-border-strong, rgba(255, 255, 255, 0.2)); background: rgba(255, 255, 255, 0.1); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--accent-focus, #4fc3ff); outline-offset: 2px; }
    &[aria-pressed='true'] { border-color: var(--accent-border, rgba(0, 171, 255, 0.55)); background: var(--accent-soft, rgba(0, 171, 255, 0.16)); color: var(--l-text); }
    &:disabled { opacity: 0.4; cursor: default; }
    &--icon { width: 30px; padding: 0; }
    i { font-size: 11px; }
  }
}

@media (prefers-reduced-motion: reduce) {
  .gv__btn { transition: none; }
}
</style>
