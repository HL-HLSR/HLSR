<template>
  <section class="gp" :class="{ 'gp--portrait': portrait }" :aria-label="$t('#L_GS_X_OVERLAY_TITLE')">
    <GhostingPreviewViewport
      ref="inline"
      class="gp__view"
      expandable
      framed
      :fill="portrait"
      :source="source"
      :frame-width="frameWidth"
      :loading="loading"
      :message="statusText"
      :label="$t('#L_GS_X_OVERLAY_TITLE')"
      :describedby="legendId"
      :motion="canAnimate"
      :animating="animate"
      :handle="overlayHandle"
      @motion="animate = !animate"
      @expand="openExpanded"
      @handle-move="moveOverlay"
      @box="stageBox = $event"
    />
    <p :id="legendId" class="gp__legend">
      <span v-if="caption" :class="{ 'is-error': assetError }">{{ caption }}</span>
      <span v-if="screenText">{{ screenText }}</span>
      <span v-for="line in sceneLines" :key="line">{{ line }}</span>
      <span>{{ $t('#L_GS_X_PREVIEW_HELP') }}</span>
    </p>

    <!-- Окно выносится в корень лаунчера: внутри панели оно попадало в её
         слой (z-index: 1) и оказывалось под шапкой и без затемнения боковой
         колонки. В корне остаются переменные темы. -->
    <Teleport v-if="expanded" to=".launcher-root">
    <div class="gp__backdrop" @mousedown.self="closeExpanded">
      <section ref="dialog" class="gp__dialog" :style="{ '--gp-aspect': frameWidth / FRAME_HEIGHT }" role="dialog" aria-modal="true" :aria-label="$t('#L_GS_X_OVERLAY_TITLE')" tabindex="-1">
        <header class="gp__head">
          <strong>{{ $t('#L_GS_X_OVERLAY_TITLE') }}</strong>
          <span class="gp__head-note" :class="{ 'is-error': assetError }">{{ caption || $t('#L_GS_X_PREVIEW_HELP') }}</span>
          <button ref="close" type="button" class="gp__close" :title="$t('#L_GS_X_PREVIEW_CLOSE')" :aria-label="$t('#L_GS_X_PREVIEW_CLOSE')" @click="closeExpanded">
            <i class="fas fa-times" aria-hidden="true"></i>
          </button>
        </header>
        <GhostingPreviewViewport
          ref="expanded"
          class="gp__dialog-view"
          :source="source"
          :frame-width="frameWidth"
          :loading="loading"
          :message="statusText"
          :label="$t('#L_GS_X_OVERLAY_TITLE')"
          :motion="canAnimate"
          :animating="animate"
          :handle="overlayHandle"
          @motion="animate = !animate"
          @handle-move="moveOverlay"
        />
      </section>
    </div>
    </Teleport>
  </section>
</template>

<script>
import GhostingPreviewViewport from './GhostingPreviewViewport';
import { loadLayers, FRAME_WIDTH, FRAME_HEIGHT } from './ghosting-overlay-renderer';
import { loadLayers as loadHl1Layers } from './hl1-overlay-renderer';
import { loadScenes } from './ghosting-scene-renderer';
import { meshSprite } from './ghost-mesh-sprite';
import { drawSample } from './livesplit-sample';
const liveSplitOverlay = require('../../../shared/livesplit-overlay');
const scenePreview = require('../../../shared/ghosting-scene-preview');
let fontId = 0;
// Resolves once a tab switch (l-slide-*) started now has finished.
function afterSwipe(el) {
  const value = getComputedStyle(el).getPropertyValue('--l-dur-tab').trim();
  const ms = parseFloat(value) * (value.endsWith('ms') ? 1 : 1000) || 0;
  return new Promise((resolve) => setTimeout(resolve, ms));
}
let legendSequence = 0;
// Кадры сняты в 16:9 при высоте 900; другое соотношение сторон меняет только
// ширину кадра. Пределы — 5:4 и 32:9.
function frameWidthOf(screen) {
  if (!screen || !screen.width || !screen.height) return FRAME_WIDTH;
  return Math.min(3200, Math.max(1125, Math.round(FRAME_HEIGHT * screen.width / screen.height)));
}
export default {
  name: 'GhostingSettingsPreview',
  components: { GhostingPreviewViewport },
  props: {
    value: { type: Object, required: true },
    section: { type: String, default: 'hud' },
    gameId: { type: String, default: '218' },
    active: { type: Boolean, default: true },
    // 'hl1': frames of Half-Life WON + BXT instead of HL2 Ghosting.
    variant: { type: String, default: 'ghosting' },
    // Оверлей LiveSplit поверх кадра: настройки, снимок настоящего окна
    // (без него — примерный LiveSplit) и можно ли таскать его мышью.
    overlay: { type: Object, default: null },
    overlaySnapshot: { type: Object, default: null },
    overlayEditing: { type: Boolean, default: false },
    // Включённый HUD-мод (custom-packs.js): кадр рисует его раскладку и шрифт цифр.
    hudMod: { type: String, default: '' },
  },
  emits: ['overlay'],
  data() { return {
      // Своя модель призрака: '' (стандартная), loading, shown, missing (нет в файлах игры), failed (не прочиталась).
      ghostModelState: '', FRAME_HEIGHT, ready: false, error: false, sceneReady: false, sceneError: false, sprayInfo: null, stageBox: null, assetError: '', animate: false, reduceMotion: false, expanded: false, screen: null, legendId: 'gp-legend-' + (++legendSequence) }; },
  computed: {
    hl1() { return this.variant === 'hl1'; },
    // Разделы со своей сценой вместо кадра с HUD: призрак на пляже и спрей на стене.
    scene() { return this.hl1 ? '' : this.section === 'ghosts' ? 'ghost' : this.section === 'misc' ? 'spray' : ''; },
    // Призрак — в узком вертикальном окне «под себя»; ширина кадра следует за окном.
    portrait() { return this.scene === 'ghost'; },
    shown() { return this.scene ? this.sceneReady : this.ready; },
    failed() { return this.scene ? this.sceneError : this.error; },
    loading() { return !this.shown && !this.failed; },
    sceneLines() {
      const extra = Object.assign({ gh_sprays: true }, this.value.extra || {}), locale = this.$t('#L_RUNNER_LOCALE');
      // Своя модель (gh_model): нарисована из файлов игры — в исходной позе; не нашлась — стандартная.
      const caption = { shown: '#L_GS_X_SCENE_GHOST_MODEL_POSE', missing: '#L_GS_X_SCENE_GHOST_MODEL', failed: '#L_GS_X_SCENE_GHOST_MODEL_FAILED' }[this.ghostModelState];
      if (this.scene === 'ghost') return [this.$t('#L_GS_X_SCENE_GHOST', Math.round(scenePreview.ghostDistance())), caption ? this.$t(caption) : ''].filter(Boolean);
      if (this.scene !== 'spray') return [];
      if (!extra.gh_sprays) return [this.$t('#L_GS_X_SCENE_SPRAYS_OFF')];
      const info = this.sprayInfo;
      const units = info && Number(info.units.toFixed(2)).toLocaleString(locale);
      return [info ? this.$t('#L_GS_X_SCENE_SPRAY_SIZE', units, units, info.texels, info.texels) : '', extra.gh_spray_file ? '' : this.$t('#L_GS_X_SCENE_SPRAY_EXAMPLE')].filter(Boolean);
    },
    // Ширина кадра по разрешению, в котором запустится игра (source-screen.js
    // в main): HUD в Source держится за края экрана, поэтому на 16:10 или
    // 21:9 он стоит иначе, чем в кадре 16:9. Неизвестно — экран монитора.
    gameScreen() {
      if (this.hl1) return null;
      if (this.screen) return this.screen;
      // window.screen is in CSS pixels; the game counts physical ones.
      const ratio = window.devicePixelRatio || 1;
      const w = window.screen && Math.round(window.screen.width * ratio), h = window.screen && Math.round(window.screen.height * ratio);
      return w && h ? { width: w, height: h, source: 'display' } : null;
    },
    frameWidth() {
      if (this.portrait) { const box = this.stageBox; return box && box.width > 0 && box.height > 0 ? Math.round(Math.min(1.2, Math.max(0.45, box.width / box.height)) * FRAME_HEIGHT) : FRAME_HEIGHT * 3 / 4; }
      return this.hl1 || this.scene ? FRAME_WIDTH : frameWidthOf(this.gameScreen);
    },
    screenText() {
      const screen = this.scene ? null : this.gameScreen;
      return screen ? this.$t('#L_GS_X_PREVIEW_SCREEN_' + screen.source.toUpperCase(), screen.width + '×' + screen.height) : '';
    },
    canAnimate() { return !this.hl1 && (this.section === 'weapon' || this.section === 'keys'); },
    // Прицел пака подчиняется тому же переключателю BXT, что и в игре.
    packHidden() { const e = this.value.extra || {}; return this.hl1 && e.packCrosshair && e.packCrosshair !== 'default' && this.value.crossEnabled && this.value.crossHideDefault; },
    statusText() { return this.failed ? this.$t('#L_GS_X_OVERLAY_ERROR') : this.shown ? '' : this.$t('#L_GS_X_OVERLAY_LOADING'); },
    overlayValue() { return this.overlay ? liveSplitOverlay.sanitizeOverlay(this.overlay) : null; },
    // Выключенный оверлей виден только в его разделе, полупрозрачным.
    overlayShown() { return !!this.overlayValue && (this.overlayValue.enabled || this.overlayEditing); },
    // Только то, от чего зависит картинка: перемещение и прозрачность её не
    // пересчитывают (Vue не будит зависимых, если строка не изменилась).
    overlayKeying() {
      if (!this.overlayShown) return '';
      const value = this.overlayValue;
      return JSON.stringify({ key: value.key, keyColor: value.keyColor, similarity: value.similarity, smoothness: value.smoothness, lumaMin: value.lumaMin, lumaMinSmooth: value.lumaMinSmooth, lumaMax: value.lumaMax, lumaMaxSmooth: value.lumaMaxSmooth, crop: value.crop });
    },
    // Картинка LiveSplit после обрезки и маски.
    overlayImage() {
      if (!this.overlayKeying) return null;
      const value = liveSplitOverlay.sanitizeOverlay(JSON.parse(this.overlayKeying));
      const snapshot = this.overlaySnapshot;
      let keyed;
      if (snapshot && snapshot.pixels) {
        keyed = liveSplitOverlay.keyImage(value, snapshot.pixels, snapshot.width, snapshot.height);
      } else {
        const sample = drawSample(this.hl1 ? 'hl1' : 'ghosting');
        const data = sample.getContext('2d').getImageData(0, 0, sample.width, sample.height).data;
        keyed = liveSplitOverlay.keyImage(value, data, sample.width, sample.height, 'rgba');
      }
      const canvas = document.createElement('canvas');
      canvas.width = keyed.width;
      canvas.height = keyed.height;
      canvas.getContext('2d').putImageData(new ImageData(keyed.data, keyed.width, keyed.height), 0, 0);
      return canvas;
    },
    overlayRect() {
      const image = this.overlayImage;
      return image ? liveSplitOverlay.placement(this.overlayValue, this.frameWidth, FRAME_HEIGHT, image.width, image.height) : null;
    },
    overlayHandle() { return this.overlayEditing ? this.overlayRect : null; },
    caption() { return this.assetError ? this.$t(this.assetError === 'decode' ? '#L_GS_X_OVERLAY_ASSET_DECODE' : '#L_GS_X_OVERLAY_ASSET_ERROR') : this.packHidden ? this.$t('#L_GS_H_PREVIEW_PACK_HIDDEN') : this.section === 'weapon' && !this.hl1 ? this.$t('#L_GS_X_OVERLAY_WEAPON') : ''; },
  },
  watch: {
    value: { deep: true, handler() { this.queueRender(); this.loadAssets(); } },
    section() { this.animate = false; this.loadScene(); this.queueRender(); },
    overlayImage() { this.queueRender(); },
    overlayRect() { this.queueRender(); },
    overlayValue: { deep: true, handler() { this.queueRender(); } },
    animate() { this.queueRender(); },
    frameWidth(width) { this.source.width = width; this.queueRender(); },
    gameId() { this.loadScreen(); },
    hudMod() { this.loadAssets(); this.queueRender(); },
    // Файл могли положить, пока окно было закрыто: при открытии — новая попытка.
    // Разрешение тоже перечитывается: игру могли запустить с другим.
    active(value) { if (!value) return; this.loadScreen(); this.loadScene(); if (this.assetError) { this.assetSignature = ''; this.loadAssets(); } this.queueRender(); },
  },
  created() {
    // The frame is drawn once into this off-screen canvas; the views only show it.
    this.source = document.createElement('canvas');
    this.source.width = this.frameWidth;
    this.source.height = FRAME_HEIGHT;
    this.frameRequest = 0;
    this.overlayAssets = {};
    this.loadedFonts = [];
    this.assetRevision = 0;
    this.assetSignature = '';
    this.previousFocus = null;
  },
  methods: {
    queueRender() {
      if (!this.active || !this.shown || this.frameRequest) return;
      this.frameRequest = requestAnimationFrame(() => { this.frameRequest = 0; this.draw(); if (this.animate && !this.reduceMotion) this.queueRender(); });
    },
    draw() {
      if (this.scene) {
        if (!this.sceneRenderer) return;
        const spray = this.sceneRenderer.render(this.source, this.value, this.scene, this.overlayAssets);
        if (JSON.stringify(spray || null) !== JSON.stringify(this.sprayInfo)) this.sprayInfo = spray || null;
      } else {
        if (!this.layerRenderer) return;
        this.layerRenderer.render(this.source, this.value, this.section, this.overlayAssets, this.animate ? (this.reduceMotion ? 0.25 : performance.now() / 1000) : 0);
        this.drawLiveSplit();
      }
      if (this.$refs.inline) this.$refs.inline.paint();
      if (this.expanded && this.$refs.expanded) this.$refs.expanded.paint();
    },
    // Тот же расчёт, что у hlsr-overlay.dll: высота — доля кадра, место —
    // доля свободного пространства, прозрачность поверх маски.
    drawLiveSplit() {
      const image = this.overlayImage, rect = this.overlayRect;
      if (!image || !rect) return;
      const ctx = this.source.getContext('2d');
      ctx.save();
      ctx.globalAlpha = this.overlayValue.opacity / 100 * (this.overlayValue.enabled ? 1 : 0.45);
      ctx.imageSmoothingEnabled = true;
      ctx.imageSmoothingQuality = 'high';
      ctx.drawImage(image, rect.left, rect.top, rect.width, rect.height);
      ctx.restore();
    },
    moveOverlay({ left, top }) {
      const rect = this.overlayRect;
      if (!rect) return;
      const position = liveSplitOverlay.positionOf(left, top, this.frameWidth, FRAME_HEIGHT, rect.width, rect.height);
      if (position.x === this.overlayValue.x && position.y === this.overlayValue.y) return;
      this.$emit('overlay', Object.assign({}, this.overlayValue, position));
    },
    loadScene() {
      if (!this.scene || this.sceneRenderer) return;
      loadScenes().then((renderer) => { if (this._isDestroyed) return; this.sceneRenderer = renderer; this.sceneReady = true; this.sceneError = false; this.queueRender(); })
        .catch(() => { if (!this._isDestroyed) this.sceneError = true; });
    },
    async loadScreen() {
      if (this.hl1 || !window.hlsrElectron) return;
      const gameId = this.gameId;
      try {
        const screen = await window.hlsrElectron.ipcRenderer.invoke('ghosting-screen-get', gameId);
        if (!this._isDestroyed && gameId === this.gameId) this.screen = screen && screen.width ? screen : null;
      } catch (error) { /* остаётся экран монитора */ }
    },
    async loadAssets() {
      if (this.hl1) return;
      const extra = this.value.extra || {};
      const signature = JSON.stringify([extra.gh_hud_font || '', extra.gh_spray_file || '', extra.gh_showfps_font || '', extra.gh_showpos_font || '', extra.gh_model || '', this.hudMod]);
      if (signature === this.assetSignature) return;
      this.assetSignature = signature;
      const revision = ++this.assetRevision;
      // The renderer only uses the assets of the newest revision.
      this.loadedFonts.forEach((face) => document.fonts.delete(face)); this.loadedFonts = [];
      const assets = {}; this.assetError = ''; this.ghostModelState = '';
      if (this.hudMod) assets.hudMod = this.hudMod;
      this.overlayAssets = assets;
      // Шрифт цифр HUD-мода лежит в его папке custom: читается тем же путём, что шрифт игрока.
      // Не прочитался — renderer берёт похожий DIN из системы, ошибки в подписи нет.
      if (this.hudMod === 'condensed-hud' && window.hlsrElectron) {
        try {
          const result = await window.hlsrElectron.ipcRenderer.invoke('game-personalization-preview-asset', this.gameId, { kind: 'font', value: 'resource/din1451alt2.ttf' });
          if (revision !== this.assetRevision || this._isDestroyed) return;
          if (result && result.status === 'ok' && result.dataUrl) {
            const family = 'HlsrPreviewFont' + (++fontId), face = new FontFace(family, `url(${result.dataUrl})`);
            await face.load();
            if (revision !== this.assetRevision || this._isDestroyed) return;
            document.fonts.add(face); this.loadedFonts.push(face); assets.hudModFont = family;
          }
        } catch (error) { /* похожий шрифт из системы */ }
      }
      // Своя модель призрака: геометрия из игры (ghost-model.js) -> серый силуэт. Стандартная
      // models/player.mdl — готовый ghost.webp (в позе Idle01). Не нашлась — тоже готовый.
      const model = extra.gh_model;
      if (model && String(model).trim().toLowerCase() !== 'models/player.mdl' && window.hlsrElectron) {
        this.ghostModelState = 'loading';
        try {
          const result = await window.hlsrElectron.ipcRenderer.invoke('game-personalization-preview-asset', this.gameId, { kind: 'model', value: model });
          if (revision !== this.assetRevision || this._isDestroyed) return;
          if (result && result.status === 'ok' && result.mesh) assets.ghostModel = meshSprite(result.mesh) || undefined;
          else if (result && result.code === 'ASSET_NOT_FOUND') this.assetError = 'missing';
          this.ghostModelState = assets.ghostModel ? 'shown' : result && result.code === 'ASSET_NOT_FOUND' ? 'missing' : 'failed';
        } catch (error) { if (revision === this.assetRevision && !this._isDestroyed) this.ghostModelState = 'failed'; }
      }
      await Promise.all(['gh_hud_font', 'gh_showfps_font', 'gh_showpos_font', 'gh_spray_file'].map(async (key) => {
        const value = extra[key]; if (!value || !window.hlsrElectron) return;
        const kind = key === 'gh_spray_file' ? 'spray' : 'font';
        let stage = 'missing';
        try {
          const result = await window.hlsrElectron.ipcRenderer.invoke('game-personalization-preview-asset', this.gameId, { kind, value });
          if (revision !== this.assetRevision || this._isDestroyed) return;
          if (!result || result.status !== 'ok' || !result.dataUrl) { if (/[\\/]|\.(ttf|otf|png|bmp|tga)$/i.test(value)) this.assetError = 'missing'; return; }
          // Файл прочитан, но Chromium не смог его отрисовать: это не «файла нет».
          stage = 'decode';
          if (kind === 'font') {
            const family = 'HlsrPreviewFont' + (++fontId);
            const face = new FontFace(family, `url(${result.dataUrl})`);
            await face.load();
            if (revision !== this.assetRevision || this._isDestroyed) return;
            document.fonts.add(face); this.loadedFonts.push(face);
            assets[key === 'gh_hud_font' ? 'fontFamily' : key.replace('_font', '')] = family;
          } else {
            const image = new Image();
            await new Promise((resolve, reject) => { image.onload = resolve; image.onerror = reject; image.src = result.dataUrl; });
            if (revision === this.assetRevision && !this._isDestroyed) assets.spray = image;
          }
          this.queueRender();
        } catch (error) { if (revision === this.assetRevision && !this._isDestroyed) this.assetError = stage; }
      }));
      this.queueRender();
    },
    openExpanded() {
      if (!this.shown) return;
      this.previousFocus = document.activeElement;
      this.expanded = true;
      this.$nextTick(() => { this.draw(); if (this.$refs.close) this.$refs.close.focus(); });
    },
    closeExpanded() {
      if (!this.expanded) return;
      this.expanded = false;
      this.$nextTick(() => { if (this.previousFocus && this.previousFocus.isConnected) this.previousFocus.focus(); this.previousFocus = null; });
    },
    // Runs before the surrounding settings window sees the key, so Escape closes
    // only the enlarged view and Tab stays inside it.
    onDialogKey(event) {
      if (!this.expanded) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); this.closeExpanded(); return; }
      if (event.key !== 'Tab' || !this.$refs.dialog) return;
      event.stopImmediatePropagation();
      const elements = this.$refs.dialog.querySelectorAll('button:not(:disabled), [tabindex="0"]');
      const first = elements[0], last = elements[elements.length - 1];
      const outside = !this.$refs.dialog.contains(document.activeElement);
      if (event.shiftKey && (document.activeElement === first || outside)) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || outside)) { event.preventDefault(); first.focus(); }
    },
  },
  mounted() {
    this.reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    // A renderer that was not prepared in idle time creates its WebGL context
    // and works out the HUD on the first frame, which blocks for a few hundred
    // ms; that waits until the tab has slid in.
    (this.hl1 ? loadHl1Layers() : loadLayers().then((renderer) => (renderer.prepared ? renderer : afterSwipe(this.$el).then(() => renderer)))).then((renderer) => { if (this._isDestroyed) return; this.layerRenderer = renderer; renderer.onLayer = () => this.queueRender(); this.ready = true; this.queueRender(); this.loadAssets(); }).catch(() => { if (!this._isDestroyed) this.error = true; });
    window.addEventListener('keydown', this.onDialogKey, true);
    this.loadScreen();
    this.loadScene();
  },
  beforeUnmount() {
    this.assetRevision++;
    cancelAnimationFrame(this.frameRequest);
    this.loadedFonts.forEach((face) => document.fonts.delete(face));
    window.removeEventListener('keydown', this.onDialogKey, true);
  },
};
</script>

<style lang="scss" scoped>
/* A card like the settings next to it: the same border, background and full
   column height, so the three columns start and end on the same lines. The
   frame is centred in whatever height is left; the dark well around it is
   part of the view, not empty space under a short image. */
// The card is as tall as its 16:9 frame, so there are no bars around it.
.gp {
  display: flex;
  flex-direction: column;
  align-self: start;
  max-height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: var(--l-modal-bar);
  color: var(--l-text);
  container: gp / inline-size;

  // Низкое окно: кадр ужимается (поля по бокам), а подпись под ним видна целиком.
  &__view { flex: 0 1 auto; }

  // Вертикальное окно: высота — вся строка, ширина — от высоты (около 3:4 у
  // самого кадра), кадр берёт место, что осталось от панели и подписи.
  &--portrait {
    align-self: stretch;
    justify-self: start;
    max-width: 100%;
    aspect-ratio: 5 / 8;
  }
  &--portrait &__view { flex: 1 1 0; }
  // Низкое окно (Full HD: 1152×648, 1200×676): 5:8 от высоты даёт полоску в 250 px
  // и пустоту справа. Тут окно берёт всю колонку, сцена сама не шире 1,2:1 (portraitWidth).
  @media (max-height: 760px) {
    &--portrait {
      width: 100%;
      aspect-ratio: auto;
    }
  }
  // Узкая колонка (вертикальное или маленькое окно с выдвинутой колонкой
  // значков): кнопки масштаба переносятся на вторую строку, а не обрезаются.
  :deep(.gv__bar) { flex-wrap: wrap; row-gap: 6px; }
  // Full HD 1152 с выдвинутой панелью (колонка 285 px): кнопки плотнее и остаются в одной строке.
  @container gp (max-width: 320px) {
    :deep(.gv--framed .gv__bar) { gap: 4px; padding-inline: 6px; }
    :deep(.gv__btn:not(.gv__btn--icon)) { padding: 0 8px; }
    :deep(.gv__level) { min-width: 40px; }
  }

  &__legend {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 2px;
    margin: 0;
    padding: 8px 12px 9px;
    border-top: 1px solid var(--l-border);
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 1.45;
    text-wrap: pretty;
  }
  .is-error { color: #efa5a5; }

  &__backdrop {
    position: absolute;
    inset: 0;
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
    box-sizing: border-box;
    padding: 20px;
    overscroll-behavior: contain;
    background: rgba(2, 6, 11, 0.88);
  }
  &__dialog {
    display: flex;
    flex-direction: column;
    // Wide enough for the frame to fill the height left after the header and
    // the toolbar (--gp-chrome), and no wider: the frame has the game's
    // aspect ratio (--gp-aspect).
    --gp-chrome: 118px;
    width: min(1500px, 100%, calc((100vh - 40px - var(--gp-chrome)) * var(--gp-aspect, 1.7778) + 26px));
    min-height: 0;
    overflow: hidden;
    border: 1px solid var(--l-modal-border, rgba(255, 255, 255, 0.1));
    border-radius: var(--l-radius, 14px);
    background: var(--l-modal-bg, #12161d);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);

    &:focus { outline: none; }
  }
  &__head {
    display: flex;
    flex: none;
    align-items: center;
    gap: 14px;
    padding: 8px 8px 8px 18px;
    border-bottom: 1px solid var(--l-modal-border, rgba(255, 255, 255, 0.1));
    background: var(--l-modal-bar, #10151c);
    font-size: 14px;

    strong { font-weight: 600; }
  }
  &__head-note { flex: 1; min-width: 0; overflow: hidden; color: var(--l-text-faint); font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
  &__head-note.is-error { color: #efa5a5; }
  &__close {
    width: 34px;
    height: 34px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--l-text-dim);
    font-size: 15px;
    cursor: pointer;
    transition: background var(--l-fast, 0.16s ease), color var(--l-fast, 0.16s ease);

    &:hover { background: rgba(255, 255, 255, 0.08); color: #fff; }
    &:focus-visible { outline: 2px solid var(--accent-focus, #4fc3ff); outline-offset: 2px; }
  }
  &__dialog-view { flex: none; box-sizing: border-box; padding: 12px; }
}

@media (prefers-reduced-motion: reduce) {
  .gp__close { transition: none; }
}
</style>
