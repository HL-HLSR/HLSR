<template>
  <!-- Плеер HLSR. С видео шкала — время клипа, без видео — время демо:
       метки источника видны и до предпросмотра, по шкале можно отметить
       интервал экспорта. -->
  <div
    ref="root"
    class="dpl"
    :class="{ 'is-expanded': expanded, 'is-full': isFullscreen, 'has-video': !!src }"
    tabindex="0"
    :aria-label="$t('#L_DEMO_TITLE')"
    @keydown="onKey"
  >
    <div class="dpl__screen" @click="onScreenClick" @dblclick="toggleFullscreen">
      <video
        v-if="src"
        ref="video"
        class="dpl__video"
        :src="src"
        preload="metadata"
        @loadedmetadata="onMeta"
        @play="onPlay"
        @pause="onPause"
        @ended="onPause"
        @seeked="syncTime"
        @volumechange="syncVolume"
        @error="$emit('error')"
      ></video>
      <div v-if="$slots.overlay" class="dpl__overlay" @click.stop @dblclick.stop>
        <slot name="overlay"></slot>
      </div>
    </div>

    <div class="dpl__bar">
      <div
        ref="track"
        class="dpl__track"
        role="slider"
        tabindex="-1"
        :aria-label="$t('#L_DEMO_TIMELINE')"
        aria-valuemin="0"
        :aria-valuemax="Math.round(length)"
        :aria-valuenow="Math.round(position)"
        :aria-valuetext="clock(position)"
        :class="{ 'is-idle': !length }"
        @pointerdown="onTrackDown"
        @pointermove="onTrackMove"
        @pointerup="onTrackUp"
        @pointercancel="onTrackUp"
        @pointerleave="hover = null"
      >
        <div class="dpl__rail"></div>
        <div v-if="rangeOnBar" class="dpl__range" :style="rangeOnBar"></div>
        <div class="dpl__played" :style="{ width: percent(position) }"></div>
        <span
          v-for="(marker, index) in markers"
          :key="index"
          class="dpl__marker"
          :class="'is-' + marker.type"
          :style="{ left: percent(marker.t) }"
          aria-hidden="true"
        ></span>
        <span class="dpl__head" :style="{ left: percent(position) }" aria-hidden="true"></span>
        <div v-if="hover" class="dpl__tip" :style="{ left: hover.left }" aria-hidden="true">
          <span v-if="hover.label" class="dpl__tip-label" :class="'is-' + hover.type">{{ hover.label }}</span>
          <span class="dpl__tip-time">{{ hover.time }}</span>
        </div>
      </div>
    </div>

    <div class="dpl__controls">
      <button type="button" class="dpl__btn" :disabled="!src" :title="$t(playing ? '#L_DEMO_PAUSE' : '#L_DEMO_PLAY')"
        :aria-label="$t(playing ? '#L_DEMO_PAUSE' : '#L_DEMO_PLAY')" @click="togglePlay">
        <i :class="playing ? 'fas fa-pause' : 'fas fa-play'" aria-hidden="true"></i>
      </button>
      <span class="dpl__time">{{ clock(position) }} <span class="dpl__time-total">/ {{ clock(length) }}</span></span>

      <span class="dpl__spacer"></span>

      <template v-if="marking">
        <button type="button" class="dpl__mark" :disabled="!length" :title="$t('#L_DEMO_SET_START')" :aria-label="$t('#L_DEMO_SET_START')" @click="mark('from')">
          <i class="fas fa-step-backward" aria-hidden="true"></i><span class="dpl__mark-label">{{ $t("#L_DEMO_SET_START") }}</span>
        </button>
        <button type="button" class="dpl__mark" :disabled="!length" :title="$t('#L_DEMO_SET_END')" :aria-label="$t('#L_DEMO_SET_END')" @click="mark('to')">
          <span class="dpl__mark-label">{{ $t("#L_DEMO_SET_END") }}</span><i class="fas fa-step-forward" aria-hidden="true"></i>
        </button>

        <span class="dpl__sep" aria-hidden="true"></span>
      </template>

      <div class="dpl__rate" ref="rate">
        <button type="button" class="dpl__btn dpl__btn--text" :disabled="!src" :title="$t('#L_DEMO_RATE')"
          :aria-label="$t('#L_DEMO_RATE')" aria-haspopup="listbox" :aria-expanded="rateOpen ? 'true' : 'false'"
          @click="rateOpen = !rateOpen">{{ rateText(rate) }}</button>
        <div v-if="rateOpen" class="dpl__menu" role="listbox" :aria-label="$t('#L_DEMO_RATE')">
          <span class="dpl__menu-title">{{ $t("#L_DEMO_RATE") }}</span>
          <button v-for="value in rates" :key="value" type="button" role="option" class="dpl__menu-item"
            :class="{ active: value === rate }" :aria-selected="value === rate ? 'true' : 'false'" @click="setRate(value)">
            {{ rateText(value) }}<i v-if="value === rate" class="fas fa-check" aria-hidden="true"></i>
          </button>
        </div>
      </div>

      <div class="dpl__volume">
        <button type="button" class="dpl__btn" :title="$t(muted ? '#L_DEMO_UNMUTE' : '#L_DEMO_MUTE')"
          :aria-label="$t(muted ? '#L_DEMO_UNMUTE' : '#L_DEMO_MUTE')" @click="toggleMute">
          <i :class="volumeIcon" aria-hidden="true"></i>
        </button>
        <input type="range" class="dpl__slider" min="0" max="1" step="0.05" :value="muted ? 0 : volume"
          :aria-label="$t('#L_DEMO_VOLUME')" :style="{ '--fill': (muted ? 0 : volume) * 100 + '%' }" @input="setVolume($event.target.value)" />
      </div>

      <button type="button" class="dpl__btn" :title="$t(isFullscreen || expanded ? '#L_DEMO_FULLSCREEN_EXIT' : '#L_DEMO_FULLSCREEN')"
        :aria-label="$t(isFullscreen || expanded ? '#L_DEMO_FULLSCREEN_EXIT' : '#L_DEMO_FULLSCREEN')" @click="toggleFullscreen">
        <i :class="isFullscreen || expanded ? 'fas fa-compress' : 'fas fa-expand'" aria-hidden="true"></i>
      </button>
    </div>
  </div>
</template>

<script>
import { RATES, formatClock, demoTimeAt, videoTimeAt } from "./demo-tool";

const { ipcRenderer } = window.hlsrElectron;

const SNAP_PX = 6;
const STEP = 5;

export default {
  name: "DemoPlayer",
  emits: ["mark", "error", "expanded"],
  props: {
    // hlsr-media:// из demo-media-url или результата задания; null — видео нет.
    src: { type: String, default: null },
    // timeline.video: { range, speed, duration } — для пересчёта во время демо.
    video: { type: Object, default: null },
    // Метки: с видео — время клипа ({ t, type, label, sourceTime }), без — время демо.
    markers: { type: Array, default: () => [] },
    // Длина шкалы без видео — длительность демо.
    duration: { type: Number, default: 0 },
    // Интервал экспорта во времени демо: { from, to } или null.
    range: { type: Object, default: null },
    markerText: { type: Function, default: (marker) => marker.label },
    // Кнопки «Начало здесь» и «Конец здесь» (интервал экспорта); у готовых видео их нет.
    marking: { type: Boolean, default: true },
  },
  data() {
    return {
      rates: RATES,
      playing: false,
      current: 0,
      cursor: 0,
      mediaDuration: 0,
      volume: 1,
      muted: false,
      rate: 1,
      rateOpen: false,
      expanded: false,
      // Окно ушло в полный экран через demo-player-fullscreen.
      windowFull: false,
      isFullscreen: false,
      hover: null,
      dragging: false,
    };
  },
  computed: {
    length() {
      if (this.src) return this.mediaDuration || (this.video && this.video.duration) || 0;
      return this.duration || 0;
    },
    position() {
      return this.src ? this.current : this.cursor;
    },
    volumeIcon() {
      if (this.muted || this.volume === 0) return "fas fa-volume-mute";
      return this.volume < 0.5 ? "fas fa-volume-down" : "fas fa-volume-up";
    },
    // Интервал экспорта на шкале; с видео — переведённый во время клипа.
    rangeOnBar() {
      if (!this.range || !this.length) return null;
      const from = this.src ? videoTimeAt(this.range.from, this.video, this.markers) : this.range.from;
      const to = this.src ? videoTimeAt(this.range.to, this.video, this.markers) : this.range.to;
      const a = Math.max(0, Math.min(this.length, from));
      const b = Math.max(0, Math.min(this.length, to));
      if (b <= a) return null;
      return { left: this.percent(a), width: ((b - a) / this.length) * 100 + "%" };
    },
  },
  watch: {
    src() {
      this.playing = false;
      this.current = 0;
      this.mediaDuration = 0;
      this.stopTicker();
    },
    duration() {
      this.cursor = 0;
    },
    // Страница поднимает свой слой над шапкой и колонкой, пока плеер на всё окно.
    expanded(value) {
      this.$emit("expanded", value);
      if (value) document.addEventListener("keydown", this.onExpandedKey, true);
      else document.removeEventListener("keydown", this.onExpandedKey, true);
    },
  },
  mounted() {
    document.addEventListener("fullscreenchange", this.onFullscreenChange);
    document.addEventListener("mousedown", this.onDocumentDown, true);
    // Канал и событие полного экрана окна могут отсутствовать в preload (старый
    // главный процесс): тогда остаётся «на всё окно».
    try {
      ipcRenderer.on("demo-player-fullscreen-changed", this.onWindowFullscreen);
      this.windowEvents = true;
    } catch (ignored) {
      this.windowEvents = false;
    }
  },
  beforeUnmount() {
    document.removeEventListener("fullscreenchange", this.onFullscreenChange);
    document.removeEventListener("mousedown", this.onDocumentDown, true);
    document.removeEventListener("keydown", this.onExpandedKey, true);
    if (this.windowEvents) ipcRenderer.removeListener("demo-player-fullscreen-changed", this.onWindowFullscreen);
    this.exitFull();
    this.stopTicker();
  },
  methods: {
    clock(seconds) {
      return formatClock(seconds, 1);
    },
    rateText(value) {
      return String(value).replace(".", this.$t("#L_DECIMAL")) + "×";
    },
    percent(t) {
      return this.length ? Math.max(0, Math.min(100, (t / this.length) * 100)) + "%" : "0%";
    },
    // Пауза снаружи (уход со страницы, смена демки).
    pause() {
      if (this.$refs.video) this.$refs.video.pause();
    },
    onMeta() {
      const video = this.$refs.video;
      this.mediaDuration = Number.isFinite(video.duration) ? video.duration : 0;
      video.volume = this.volume;
      video.muted = this.muted;
      video.playbackRate = this.rate;
      this.current = video.currentTime;
    },
    onPlay() {
      this.playing = true;
      this.startTicker();
    },
    onPause() {
      this.playing = false;
      this.stopTicker();
      this.syncTime();
    },
    // Головка шкалы идёт по кадрам монитора: timeupdate приходит 4 раза в секунду.
    startTicker() {
      this.stopTicker();
      const tick = () => {
        this.syncTime();
        this.ticker = requestAnimationFrame(tick);
      };
      this.ticker = requestAnimationFrame(tick);
    },
    stopTicker() {
      if (this.ticker) cancelAnimationFrame(this.ticker);
      this.ticker = null;
    },
    syncTime() {
      if (this.$refs.video && !this.dragging) this.current = this.$refs.video.currentTime;
    },
    syncVolume() {
      const video = this.$refs.video;
      if (!video) return;
      this.muted = video.muted;
      this.volume = video.volume;
    },
    togglePlay() {
      const video = this.$refs.video;
      if (!video) return;
      if (video.paused || video.ended) video.play().catch(() => {});
      else video.pause();
    },
    onScreenClick() {
      this.rateOpen = false;
      this.togglePlay();
      this.$refs.root.focus({ preventScroll: true });
    },
    seek(t) {
      const value = Math.max(0, Math.min(this.length, t));
      if (this.src) {
        if (this.$refs.video) this.$refs.video.currentTime = value;
        this.current = value;
      } else this.cursor = value;
    },
    timeAt(event) {
      const rect = this.$refs.track.getBoundingClientRect();
      const x = Math.max(0, Math.min(rect.width, event.clientX - rect.left));
      return { t: rect.width ? (x / rect.width) * this.length : 0, x, width: rect.width };
    },
    // Метка ближе SNAP_PX к курсору — щелчок и подсказка берут её.
    markerNear(x, width) {
      let best = null;
      for (const marker of this.markers) {
        const distance = Math.abs((marker.t / this.length) * width - x);
        if (distance <= SNAP_PX && (!best || distance < best.distance)) best = { marker, distance };
      }
      return best ? best.marker : null;
    },
    onTrackDown(event) {
      if (!this.length || event.button !== 0) return;
      const { t, x, width } = this.timeAt(event);
      const near = this.markerNear(x, width);
      this.dragging = true;
      this.$refs.track.setPointerCapture(event.pointerId);
      this.seek(near ? near.t : t);
      this.$refs.root.focus({ preventScroll: true });
    },
    onTrackMove(event) {
      if (!this.length) return;
      const { t, x, width } = this.timeAt(event);
      if (this.dragging) this.seek(t);
      const near = this.dragging ? null : this.markerNear(x, width);
      this.hover = {
        left: (x / width) * 100 + "%",
        time: this.clock(near ? near.t : t),
        // Все метки этой точки: на одном стыке бывают карта, глава и таймер.
        label: near ? this.markers.filter((marker) => marker.t === near.t).map((marker) => this.markerText(marker)).join(" · ") : "",
        type: near ? near.type : "",
      };
    },
    onTrackUp(event) {
      if (!this.dragging) return;
      this.dragging = false;
      if (this.$refs.track.hasPointerCapture(event.pointerId)) this.$refs.track.releasePointerCapture(event.pointerId);
      this.syncTime();
    },
    // Время демо в текущей точке: у клипа — через интервал, скорость и метки.
    mark(which) {
      const demoTime = this.src ? demoTimeAt(this.current, this.video, this.markers) : this.cursor;
      this.$emit("mark", which, Math.round(demoTime * 1000) / 1000);
    },
    // Переход по метке из списка карточки: t — время демо.
    seekDemo(demoTime) {
      this.seek(this.src ? videoTimeAt(demoTime, this.video, this.markers) : demoTime);
    },
    setRate(value) {
      this.rate = value;
      this.rateOpen = false;
      if (this.$refs.video) this.$refs.video.playbackRate = value;
    },
    setVolume(value) {
      this.volume = Number(value);
      this.muted = this.volume === 0;
      if (this.$refs.video) {
        this.$refs.video.volume = this.volume;
        this.$refs.video.muted = this.muted;
      }
    },
    toggleMute() {
      this.muted = !this.muted;
      if (!this.muted && this.volume === 0) this.volume = 0.5;
      if (this.$refs.video) {
        this.$refs.video.muted = this.muted;
        this.$refs.video.volume = this.volume;
      }
    },
    // Во весь экран: плеер занимает всё окно (класс is-expanded), а главный
    // процесс разворачивает окно на экран (demo-player-fullscreen). Нет канала
    // или он ответил false — плеер на всё окно через HTML Fullscreen
    // (окно лаунчера setFullScreenable(false) на экран не уходит), при его
    // отказе — тем же классом.
    async toggleFullscreen() {
      if (this.isFullscreen || this.expanded) return this.exitFull();
      if (this.switching) return;
      this.switching = true;
      try {
        if (await this.setWindowFullscreen(true)) {
          this.windowFull = true;
          this.expanded = true;
          return;
        }
        const root = this.$refs.root;
        await (root.requestFullscreen ? root.requestFullscreen() : Promise.reject(new Error("no api"))).catch(() => {
          this.expanded = true;
        });
      } finally {
        this.switching = false;
      }
    },
    async setWindowFullscreen(on) {
      try {
        return (await ipcRenderer.invoke("demo-player-fullscreen", on)) === true;
      } catch (error) {
        return false;
      }
    },
    // Выход: Esc, кнопка, уход со страницы.
    exitFull() {
      if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
      this.expanded = false;
      if (this.windowFull) {
        this.windowFull = false;
        this.setWindowFullscreen(false);
      }
    },
    // Окно вышло из полного экрана само (Esc в главном процессе, Win+↓): плеер на место.
    onWindowFullscreen(event, value) {
      if (value === false && this.windowFull) {
        this.windowFull = false;
        this.expanded = false;
      }
    },
    onExpandedKey(event) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      if (this.rateOpen) this.rateOpen = false;
      else this.exitFull();
    },
    onFullscreenChange() {
      this.isFullscreen = document.fullscreenElement === this.$refs.root;
    },
    onDocumentDown(event) {
      if (this.rateOpen && this.$refs.rate && !this.$refs.rate.contains(event.target)) this.rateOpen = false;
    },
    // Клавиши по event.code: F и K работают и в русской раскладке. Сочетания
    // с Ctrl/Alt/Win (Ctrl+F и т. п.) плееру не принадлежат.
    onKey(event) {
      const tag = event.target.tagName;
      if (tag === "INPUT" || (tag === "BUTTON" && (event.code === "Space" || event.code === "Enter" || event.code === "NumpadEnter"))) return;
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      const step = event.shiftKey ? 1 : STEP;
      switch (event.code) {
        case "Space":
        case "KeyK":
          this.togglePlay();
          break;
        case "ArrowLeft":
          this.seek(this.position - step);
          break;
        case "ArrowRight":
          this.seek(this.position + step);
          break;
        case "ArrowUp":
          this.setVolume(Math.min(1, (this.muted ? 0 : this.volume) + 0.1).toFixed(2));
          break;
        case "ArrowDown":
          this.setVolume(Math.max(0, (this.muted ? 0 : this.volume) - 0.1).toFixed(2));
          break;
        case "Home":
          this.seek(0);
          break;
        case "End":
          this.seek(this.length);
          break;
        case "KeyF":
          this.toggleFullscreen();
          break;
        case "Escape":
          if (!this.rateOpen) return;
          this.rateOpen = false;
          break;
        default:
          return;
      }
      event.preventDefault();
      // Стрелки не уходят в общий обработчик окна (там свои сочетания).
      event.stopPropagation();
    },
  },
};
</script>

<style lang="scss" scoped>
.dpl {
  position: relative;
  display: flex;
  flex-direction: column;
  min-height: 0;
  border-radius: var(--l-radius-sm);
  background: #000;
  box-shadow: inset 0 0 0 1px var(--l-border);
  overflow: hidden;
  outline: none;

  &:focus-visible { box-shadow: inset 0 0 0 2px var(--l-accent); }

  /* На всё окно: и через HTML Fullscreen (:fullscreen), и запасным классом. */
  &.is-expanded {
    position: fixed;
    inset: 0;
    z-index: 80;
    border-radius: 0;
  }

  /* Шапка окна перетаскивает окно (app-region: drag) и под плеером. */
  &.is-expanded,
  &:fullscreen { -webkit-app-region: no-drag; }

  &:fullscreen { border-radius: 0; }

  &__screen {
    position: relative;
    flex: 1 1 auto;
    min-height: 120px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #000;
  }

  &.has-video &__screen { cursor: pointer; }

  &__video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  /* safe center: блок ровно посередине экрана видео, а выше высоты экрана — с прокруткой от верха (не обрезается). */
  &__overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: safe center;
    justify-content: safe center;
    padding: 24px;
    overflow-y: auto;
    overscroll-behavior: contain;
    background: radial-gradient(ellipse at center, rgba(var(--l-panel-rgb, 20, 26, 36), 0.55), rgba(0, 0, 0, 0.88));
    cursor: default;
  }

  &__bar {
    flex: none;
    padding: 10px 14px 2px;
    background: var(--l-modal-bar);
  }

  &__track {
    position: relative;
    height: 20px;
    cursor: pointer;
    touch-action: none;
    outline: none;

    &.is-idle { cursor: default; }
  }

  &__rail,
  &__played,
  &__range {
    position: absolute;
    top: 50%;
    left: 0;
    height: 4px;
    transform: translateY(-50%);
    border-radius: 2px;
    pointer-events: none;
  }

  &__rail { right: 0; background: rgba(255, 255, 255, 0.14); }
  &__played { background: var(--accent); }

  /* Интервал экспорта: широкая полоса под головкой. */
  &__range {
    height: 12px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 55%, transparent);
  }

  &__marker {
    position: absolute;
    top: 50%;
    width: 3px;
    height: 14px;
    transform: translate(-50%, -50%);
    border-radius: 2px;
    background: var(--l-text-dim);
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.6);
    pointer-events: none;

    &.is-map { background: var(--accent); height: 16px; }
    &.is-saveload { background: #ffb547; }
    &.is-timer { background: #5fd17f; }
    &.is-chapter { background: #b98cff; height: 16px; }
    &.is-finish { background: #ff6b6b; height: 16px; }
  }

  &__head {
    position: absolute;
    top: 50%;
    width: 12px;
    height: 12px;
    transform: translate(-50%, -50%);
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.6);
    pointer-events: none;
  }

  &__tip {
    position: absolute;
    bottom: calc(100% + 6px);
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    border-radius: 6px;
    background: var(--l-surface-solid);
    box-shadow: inset 0 0 0 1px var(--l-border), 0 6px 18px rgba(0, 0, 0, 0.5);
    white-space: nowrap;
    pointer-events: none;
    z-index: 2;
  }

  &__tip-label {
    color: var(--l-text);
    font-size: 12px;
    font-weight: 600;

    &.is-map { color: var(--accent); }
    &.is-saveload { color: #ffb547; }
    &.is-timer { color: #5fd17f; }
    &.is-chapter { color: #b98cff; }
    &.is-finish { color: #ff6b6b; }
  }

  &__tip-time {
    color: var(--l-text-dim);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  &__controls {
    flex: none;
    // Узкое окно (Full HD 125 %, 1152 px): у «Начало здесь» и «Конец здесь» остаются значки.
    container-type: inline-size;
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 48px;
    padding: 2px 10px 8px;
    background: var(--l-modal-bar);
  }

  &__spacer { flex: 1 1 auto; }

  &__sep {
    flex: none;
    width: 1px;
    height: 20px;
    margin: 0 4px;
    background: var(--l-border);
  }

  &__time {
    flex: none;
    margin-left: 4px;
    color: var(--l-text);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  &__time-total { color: var(--l-text-faint); }

  &__btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 34px;
    height: 34px;
    padding: 0;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 14px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 0; }
    &:disabled { opacity: 0.4; cursor: default; }

    &--text {
      min-width: 48px;
      padding: 0 8px;
      font-size: 12.5px;
      font-weight: 600;
      font-variant-numeric: tabular-nums;
    }
  }

  @container (max-width: 560px) {
    &__mark-label { display: none; }
  }

  &__mark {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 11px;
    border: 1px solid var(--l-border);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text-dim);
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--l-fast), border-color var(--l-fast), color var(--l-fast);

    i { font-size: 10px; }
    &:hover:not(:disabled) { border-color: var(--l-border-strong); background: rgba(255, 255, 255, 0.1); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 1px; }
    &:disabled { opacity: 0.4; cursor: default; }
  }

  &__rate { position: relative; flex: none; }

  &__menu {
    position: absolute;
    right: 0;
    bottom: calc(100% + 8px);
    z-index: 3;
    display: flex;
    flex-direction: column;
    min-width: 168px;
    padding: 6px;
    border-radius: var(--l-radius-sm);
    background: var(--l-surface-solid);
    box-shadow: inset 0 0 0 1px var(--l-border), 0 12px 32px rgba(0, 0, 0, 0.55);
  }

  &__menu-title {
    padding: 4px 8px 6px;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  &__menu-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 30px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    cursor: pointer;

    i { color: var(--accent); font-size: 11px; }
    &:hover { background: rgba(255, 255, 255, 0.07); color: var(--l-text); }
    &.active { color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: -2px; }
  }

  &__volume {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
  }

  &__slider {
    flex: none;
    width: 76px;
    height: 20px;
    margin: 0 6px 0 0;
    background: transparent;
    appearance: none;
    cursor: pointer;

    &::-webkit-slider-runnable-track {
      height: 4px;
      border-radius: 2px;
      background: linear-gradient(90deg, var(--l-text-dim) var(--fill), rgba(255, 255, 255, 0.14) var(--fill));
    }

    &::-webkit-slider-thumb {
      width: 12px;
      height: 12px;
      margin-top: -4px;
      border-radius: 50%;
      background: #fff;
      appearance: none;
    }

    &:focus { outline: none; }
    &:focus-visible::-webkit-slider-thumb { box-shadow: 0 0 0 3px var(--accent-soft); }
  }
}

@media (max-width: 1400px) {
  .dpl__volume .dpl__slider { width: 56px; }
}
</style>
