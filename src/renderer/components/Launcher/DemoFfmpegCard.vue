<template>
  <!-- Скачиваемый инструмент «Демо»: FFmpeg (предпросмотр и экспорт, на Linux —
       из PATH) или bxt-rs (рендер HL1, GPL-3, со ссылкой на исходник).
       Закреплённую сборку и sha256 сверяет главный процесс. -->
  <div class="dff" role="group" :aria-label="title">
    <i class="fas fa-download dff__icon" aria-hidden="true"></i>
    <div class="dff__body">
      <span class="dff__title">{{ title }}</span>
      <template v-if="installable">
        <span class="dff__text">{{ $t(text.size, sizeMb) }}</span>
        <span class="dff__text dff__text--faint">{{ $t(text.why) }}</span>
        <span v-if="status.sourceLink || status.licenseLink" class="dff__links">
          <button v-if="status.sourceLink" type="button" class="dff__link" @click="open(status.sourceLink)">
            <i class="fas fa-code" aria-hidden="true"></i>{{ $t("#L_DEMO_BXTRS_SOURCE") }}
          </button>
          <button v-if="status.licenseLink" type="button" class="dff__link" @click="open(status.licenseLink)">
            <i class="fas fa-file-alt" aria-hidden="true"></i>{{ $t("#L_DEMO_BXTRS_LICENSE") }}
          </button>
        </span>
      </template>
      <span v-else class="dff__text">{{ $t(kind === "bxtrs" ? "#L_DEMO_ERR_RENDER_NOT_READY" : "#L_DEMO_FFMPEG_LINUX") }}</span>
      <span v-if="problem" class="dff__error" role="alert">{{ problem }}</span>

      <div v-if="installing" class="dff__progress">
        <div class="dff__progress-head">
          <span>{{ $t(text.installing) }}</span>
          <span class="dff__percent">{{ percent }}%</span>
        </div>
        <div class="dff__bar"><div class="dff__fill" :style="{ width: percent + '%' }"></div></div>
      </div>

      <div v-if="installable" class="dff__actions">
        <button v-if="installing" type="button" class="dff__btn" @click="$emit('cancel')">
          <i class="fas fa-times" aria-hidden="true"></i>{{ $t("#L_DEMO_JOB_CANCEL") }}
        </button>
        <button v-else type="button" class="dff__btn primary" @click="$emit('install')">
          <i class="fas fa-download" aria-hidden="true"></i>{{ $t(broken ? "#L_DEMO_FFMPEG_REINSTALL" : text.install) }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import { errorMessage } from "./demo-tool";
import { formatSize } from "./file-format";

const TEXTS = {
  ffmpeg: { title: "#L_DEMO_FFMPEG_TITLE", size: "#L_DEMO_FFMPEG_TEXT", why: "#L_DEMO_FFMPEG_WHY", install: "#L_DEMO_FFMPEG_INSTALL", installing: "#L_DEMO_FFMPEG_INSTALLING" },
  bxtrs: { title: "#L_DEMO_BXTRS_TITLE", size: "#L_DEMO_BXTRS_TEXT", why: "#L_DEMO_BXTRS_WHY", install: "#L_DEMO_BXTRS_INSTALL", installing: "#L_DEMO_BXTRS_INSTALLING" },
};

export default {
  name: "DemoFfmpegCard",
  emits: ["install", "cancel"],
  props: {
    // FfmpegStatus (demo-ffmpeg-status) или статус bxt-rs (demo-bxtrs-status).
    status: { type: Object, required: true },
    kind: { type: String, default: "ffmpeg" },
    // Код последней неудачной установки.
    error: { type: String, default: null },
  },
  computed: {
    text() {
      return TEXTS[this.kind] || TEXTS.ffmpeg;
    },
    installable() {
      return this.status.installable !== false;
    },
    installing() {
      return this.status.state === "installing";
    },
    broken() {
      return this.status.state === "error";
    },
    percent() {
      return Math.max(0, Math.min(100, Math.round(this.status.percent || 0)));
    },
    // Размер как везде в лаунчере (formatSize, двоичные единицы).
    sizeMb() {
      return formatSize(this.status.sizeBytes || 0, this.$t);
    },
    title() {
      return this.$t(this.text.title, this.installable ? this.status.pinnedVersion || "9.0.2" : "").trim();
    },
    problem() {
      const code = this.error || (this.broken ? this.status.code : null);
      if (!code || code === "CANCELLED") return "";
      const message = errorMessage({ code });
      return this.$t(message.key, ...message.args);
    },
  },
  methods: {
    // Ссылка по ключу из src/shared/links.js: адрес знает главный процесс.
    open(key) {
      window.hlsrElectron.ipcRenderer.invoke("open-external-link", key).catch(() => {});
    },
  },
};
</script>

<style lang="scss" scoped>
.dff {
  display: flex;
  gap: 14px;
  width: 100%;
  max-width: 460px;
  padding: 16px 18px;
  border-radius: var(--l-radius-sm);
  background: var(--l-modal-bg);
  box-shadow: inset 0 0 0 1px var(--l-modal-border);
  text-align: left;

  &__icon {
    flex: none;
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 14px;
  }

  &__body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  &__title {
    color: var(--l-text);
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;
  }

  &__text {
    color: var(--l-text-dim);
    font-size: 12.5px;
    line-height: 1.45;

    &--faint { color: var(--l-text-faint); }
  }

  &__links {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
  }

  &__link {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;

    i { font-size: 11px; }
    &:hover { text-decoration: underline; }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
  }

  &__error {
    color: var(--l-level-error);
    font-size: 12.5px;
    line-height: 1.45;
  }

  &__progress {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 6px;
  }

  &__progress-head {
    display: flex;
    justify-content: space-between;
    color: var(--l-text-dim);
    font-size: 12px;
  }

  &__percent { font-variant-numeric: tabular-nums; }

  &__bar {
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }

  &__fill {
    height: 100%;
    background: var(--accent);
    transition: width var(--l-fast);
  }

  &__actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
  }

  &__btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 14px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast);

    i { font-size: 12px; }
    &:hover { background: rgba(255, 255, 255, 0.1); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }

    &.primary {
      border-color: transparent;
      background: var(--accent);
      color: var(--accent-contrast-text);
      &:hover { background: var(--accent-hover); }
    }
  }
}
</style>
