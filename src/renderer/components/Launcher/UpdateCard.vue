<template>
  <transition name="l-pop">
    <section v-if="visible" class="update-card" role="status">
      <div class="update-card__head">
        <span class="update-card__icon">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M12 4V1L8 5l4 4V6c3.31 0 6 2.69 6 6 0 1.01-.25 1.97-.7 2.8l1.46 1.46A7.93 7.93 0 0 0 20 12c0-4.42-3.58-8-8-8zm0 14c-3.31 0-6-2.69-6-6 0-1.01.25-1.97.7-2.8L5.24 7.74A7.93 7.93 0 0 0 4 12c0 4.42 3.58 8 8 8v3l4-4-4-4v3z"
              fill="currentColor"
            />
          </svg>
        </span>
        <div class="update-card__texts">
          <span class="update-card__title">{{ title }}</span>
          <span class="update-card__subtitle">{{ subtitle }}</span>
        </div>
        <button
          v-if="canDismiss"
          type="button"
          class="update-card__dismiss"
          :title="$t('#L_U_LATER')"
          :aria-label="$t('#L_U_LATER')"
          @click="$emit('dismiss')"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
              fill="currentColor"
            />
          </svg>
        </button>
      </div>

      <div v-if="isDownloading" class="update-card__progress">
        <div class="update-card__track">
          <div
            class="update-card__fill"
            :style="{ width: percent + '%' }"
          ></div>
        </div>
        <div class="update-card__meta">
          <span>{{ transferredText }}</span>
          <span>{{ speedText }}</span>
          <span v-if="etaText">{{ etaText }}</span>
          <span class="update-card__percent">{{ percent }}%</span>
        </div>
      </div>

      <p v-if="notes" class="update-card__notes">{{ notes }}</p>

      <div class="update-card__actions">
        <button v-if="isDownloading || (state === 'verifying' && status.cancellable)" type="button" class="update-card__btn" @click="$emit('cancel')">
          {{ $t('#L_CANCEL') }}
        </button>
        <button
          v-if="state === 'available'"
          type="button"
          class="update-card__btn primary"
          @click="$emit('download')"
        >
          {{ $t("#L_U_DOWNLOAD") }}
        </button>

        <button
          v-if="state === 'available' && notes"
          type="button"
          class="update-card__btn"
          @click="$emit('whats-new')"
        >
          {{ $t("#L_N_WHATS_NEW") }}
        </button>
        <button
          v-if="state === 'available'"
          type="button"
          class="update-card__btn"
          @click="$emit('dismiss')"
        >
          {{ $t("#L_U_LATER") }}
        </button>

        <button
          v-if="state === 'downloaded'"
          type="button"
          class="update-card__btn primary"
          @click="$emit('install-now')"
        >
          {{ $t("#L_U_RESTART_INSTALL") }}
        </button>
        <button
          v-if="state === 'downloaded'"
          type="button"
          class="update-card__btn"
          @click="$emit('install-on-quit')"
        >
          {{ $t("#L_U_INSTALL_ON_QUIT") }}
        </button>

        <button
          v-if="state === 'error'"
          type="button"
          class="update-card__btn"
          @click="$emit('retry')"
        >
          {{ $t("#UI_RETRY") }}
        </button>
      </div>
    </section>
  </transition>
</template>

<script>
export default {
  name: "UpdateCard",
  emits: ["cancel", "dismiss", "download", "install-now", "install-on-quit", "retry", "whats-new"],
  props: {
    status: {
      type: Object,
      required: true,
    },
  },
  computed: {
    state() {
      return this.status.state;
    },
    visible() {
      return (
        ["available", "downloading", "verifying", "downloaded"].includes(this.state) ||
        (["checking", "error"].includes(this.state) && this.status.checkSource === "manual")
      );
    },
    canDismiss() {
      return this.state === "available" || this.state === "error";
    },
    isDownloading() {
      return this.state === "downloading";
    },
    percent() {
      let p = this.status.progress;
      return p && p.percent ? Math.round(p.percent) : 0;
    },
    notes() {
      if (this.state !== "available" && this.state !== "downloaded") return "";
      return (this.status.releaseNotes || "").slice(0, 400);
    },
    title() {
      switch (this.state) {
        case "checking":
          return this.$t("#L_U_CHECKING");
        case "available":
          return this.$t("#L_U_AVAILABLE", this.status.version);
        case "downloading":
          return this.$t("#L_U_DOWNLOADING");
        case "verifying":
          return this.$t("#L_B_VERIFYING");
        case "downloaded":
          return this.$t("#L_U_READY", this.status.version);
        case "error":
          return this.$t("#L_U_ERROR");
        default:
          return "";
      }
    },
    subtitle() {
      if (this.state === "error") return this.errorText;
      return this.$t("#L_U_CURRENT", this.status.currentVersion);
    },
    errorText() {
      const known = [
        "NETWORK",
        "RATE_LIMIT",
        "FEED",
        "NO_SPACE",
        "ACCESS",
        "VERIFY",
        "METADATA",
        "TIMEOUT",
        "CONFIG",
      ];
      const code = this.status.errorCode;

      return known.indexOf(code) === -1
        ? this.$t("#L_U_ERR_UNKNOWN")
        : this.$t(`#L_U_ERR_${code}`);
    },
    transferredText() {
      let p = this.status.progress;
      if (!p) return "";
      return `${this.mb(p.transferred)} / ${this.mb(p.total)}`;
    },
    speedText() {
      let p = this.status.progress;
      if (!p || !p.bytesPerSecond) return "";
      return this.$t("#L_U_SPEED", this.mb(p.bytesPerSecond));
    },
    etaText() {
      let p = this.status.progress;
      if (!p || !p.bytesPerSecond || !p.total) return "";

      let left = (p.total - p.transferred) / p.bytesPerSecond;
      if (!isFinite(left) || left <= 0) return "";

      let minutes = Math.floor(left / 60);
      let seconds = Math.round(left % 60);

      return this.$t(
        "#L_U_ETA",
        minutes > 0 ? `${minutes} ${this.$t("#L_U_MIN")} ${seconds}` : seconds
      );
    },
  },
  methods: {
    mb(bytes) {
      if (!bytes) return "0";
      return (bytes / 1024 / 1024).toFixed(1).replace(".", ",");
    },
  },
};
</script>

<style lang="scss" scoped>
.update-card {
  position: absolute;
  right: 20px;
  top: 52px;
  width: 360px;
  padding: 16px;
  background: var(--l-surface-solid);
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius);
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.5);
  z-index: 45;
  -webkit-app-region: no-drag;

  &__head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  &__icon {
    width: 34px;
    height: 34px;
    min-width: 34px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;

    svg {
      display: block;
      width: 18px;
      height: 18px;
    }
  }

  &__texts {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  &__title {
    color: var(--l-text);
    font-size: 13.5px;
    font-weight: 600;
    line-height: 1.35;
  }

  &__subtitle {
    color: var(--l-text-faint);
    font-size: 11.5px;
    line-height: 1.4;
    word-break: break-word;
  }

  &__dismiss {
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--l-text-faint);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background var(--l-fast), color var(--l-fast);

    svg {
      display: block;
      width: 14px;
      height: 14px;
    }

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      color: var(--l-text);
    }
  }

  &__progress {
    margin-top: 14px;
  }

  &__track {
    height: 5px;
    background: rgba(255, 255, 255, 0.09);
    border-radius: 3px;
    overflow: hidden;
  }

  &__fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width var(--l-med);
  }

  &__meta {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 7px;
    color: var(--l-text-dim);
    font-size: 11.5px;
  }

  &__percent {
    margin-left: auto;
    color: var(--l-text);
  }

  &__notes {
    margin-top: 12px;
    max-height: 96px;
    overflow-y: auto;
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-line;
  }

  &__actions {
    display: flex;
    gap: 8px;
    margin-top: 14px;

    &:empty {
      display: none;
    }
  }

  &__btn {
    flex: 1;
    padding: 9px 14px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font-family: inherit;
    font-size: 12.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast), transform var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.1);
    }

    &:active {
      transform: scale(0.97);
    }

    &.primary {
      background: var(--accent);
      border-color: transparent;
      color: var(--accent-contrast-text);

      &:hover {
        background: var(--accent-hover);
      }

      &:active {
        background: var(--accent-pressed);
      }
    }
  }
}
</style>
