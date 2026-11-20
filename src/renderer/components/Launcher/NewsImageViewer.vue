<template>
  <teleport to=".launcher-root">
    <div class="news-viewer" @click.self="$emit('close')">
      <section ref="dialog" class="news-viewer__dialog" role="dialog" aria-modal="true" :aria-label="caption" tabindex="-1">
        <header><span>{{ caption }}</span><span aria-live="polite">{{ index + 1 }} / {{ images.length }}</span><button ref="close" type="button" :aria-label="$t('#L_HOME_IMAGE_CLOSE')" @click="$emit('close')">×</button></header>
        <div class="news-viewer__frame">
          <p v-if="failed" role="status">{{ $t('#L_HOME_IMAGE_FAILED') }}</p>
          <img v-else :key="images[index].url" :src="images[index].url" :alt="caption + ' · ' + (index + 1)" referrerpolicy="no-referrer" @error="failed = true" />
        </div>
        <nav v-if="images.length > 1">
          <button type="button" :disabled="index === 0" @click="move(-1)">‹ {{ $t('#L_HOME_IMAGE_PREV') }}</button>
          <button type="button" :disabled="index === images.length - 1" @click="move(1)">{{ $t('#L_HOME_IMAGE_NEXT') }} ›</button>
        </nav>
      </section>
    </div>
  </teleport>
</template>

<script>
export default {
  name: 'NewsImageViewer', emits: ['close'],
  props: { images: { type: Array, required: true }, initial: { type: Number, default: 0 }, caption: String },
  data() { return { index: this.initial, failed: false }; },
  mounted() { this.previousFocus = document.activeElement; this.$refs.close.focus(); window.addEventListener('keydown', this.onKey, true); },
  beforeUnmount() { window.removeEventListener('keydown', this.onKey, true); if (this.previousFocus && this.previousFocus.isConnected) this.previousFocus.focus(); },
  methods: {
    move(d) { this.index = Math.max(0, Math.min(this.images.length - 1, this.index + d)); this.failed = false; },
    onKey(event) {
      if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); this.$emit('close'); }
      else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); this.move(event.key === 'ArrowLeft' ? -1 : 1); }
      else if (event.key === 'Tab') {
        const buttons = [...this.$refs.dialog.querySelectorAll('button:not(:disabled)')];
        const next = (buttons.indexOf(document.activeElement) + (event.shiftKey ? -1 : 1) + buttons.length) % buttons.length;
        event.preventDefault(); buttons[next].focus();
      }
    },
  },
};
</script>

<style lang="scss" scoped>
.news-viewer { position: fixed; inset: 0; z-index: 10000; display: grid; place-items: center; padding: 28px; background: rgba(5,8,11,.88); backdrop-filter: blur(8px); color: var(--l-text);
  &__dialog { display: flex; flex-direction: column; width: min(1180px, 100%); max-height: 100%; min-height: 0; border: 1px solid var(--l-border-strong); border-radius: 10px; background: var(--l-surface-solid, #20252b); overflow: hidden; }
  header { display: flex; align-items: center; gap: 20px; flex: none; padding: 12px 16px; font-size: 13px; line-height: 20px; span:first-child { flex: 1; overflow-wrap: anywhere; } }
  &__frame { display: grid; place-items: center; min-height: 0; background: #0b0e11; img { display: block; width: 100%; max-height: calc(100vh - 182px); object-fit: contain; } p { padding: 48px; } }
  nav { display: flex; justify-content: space-between; flex: none; padding: 12px 16px; gap: 12px; }
  button { padding: 6px 12px; border: 1px solid var(--l-border); border-radius: 5px; background: transparent; color: inherit; font: inherit; cursor: pointer; &:hover:not(:disabled) { background: rgba(255,255,255,.08); } &:disabled { opacity: .35; cursor: default; } &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; } }
  header button { font-size: 24px; line-height: 24px; }
}
</style>
