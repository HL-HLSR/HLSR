<template>
  <div class="settings-modal" @mousedown.self="$emit('close')">
    <div
      class="settings-modal__window unavail__window"
      role="dialog"
      aria-modal="true"
      :aria-label="$t('#L_UA_TITLE')"
      ref="window"
    >
      <header class="settings-modal__header">
        <span class="settings-modal__title">{{ $t("#L_UA_TITLE") }}</span>
        <button
          type="button"
          class="settings-modal__close"
          :title="$t('#L_CLOSE')"
          :aria-label="$t('#L_CLOSE')"
          @click="$emit('close')"
        >
          <i class="fas fa-times"></i>
        </button>
      </header>

      <div class="unavail__body">
        <div class="unavail__head">
          <i class="unavail__icon" :class="view.icon" aria-hidden="true"></i>
          <div class="unavail__texts">
            <span class="unavail__reason">{{ $t(view.title) }}</span>
            <span class="unavail__explain">{{ $t(view.text) }}</span>
          </div>
        </div>


        <details class="unavail__details">
          <summary>{{ $t("#L_UA_DETAILS") }}</summary>
          <code>{{ reason }}</code>
        </details>

        <div class="unavail__actions">
          <button
            v-for="action in actions"
            :key="action.id"
            type="button"
            class="settings-btn"
            @click="$emit('action', action.id)"
          >
            <i :class="action.icon" aria-hidden="true"></i>
            {{ $t(action.key) }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script>
import uiState from "./ui-state";
import { trapFocus } from "./focus-trap";

const ACTION = {
  recheck: { id: "recheck", icon: "fas fa-sync-alt", key: "#L_UA_ACT_RECHECK" },
  openSteam: { id: "open-steam", icon: "fab fa-steam", key: "#L_UA_ACT_OPEN_STEAM" },
  installSdk: { id: "install-sdk", icon: "fas fa-download", key: "#L_UA_ACT_SDK" },
  library: { id: "open-library-settings", icon: "fas fa-folder-open", key: "#L_UA_ACT_LIBRARY" },
  verify: { id: "verify-files", icon: "fas fa-clipboard-check", key: "#L_UA_ACT_VERIFY" },
  repair: { id: "repair", icon: "fas fa-wrench", key: "#L_UA_ACT_REPAIR" },
  diagnostics: { id: "open-diagnostics", icon: "fas fa-stethoscope", key: "#L_UA_ACT_DIAG" },
  discord: { id: "open-discord", icon: "fab fa-discord", key: "#L_UA_ACT_DISCORD" },
  updateLauncher: { id: "check-launcher-update", icon: "fas fa-arrow-up", key: "#L_UA_ACT_UPDATE_LAUNCHER" },
};

const VIEWS = {
  STEAM_NOT_RUNNING: {
    title: "#L_UA_STEAM_NOT_RUNNING",
    text: "#L_UA_STEAM_NOT_RUNNING_T",
    icon: "fab fa-steam",
    actions: [ACTION.openSteam, ACTION.recheck, ACTION.diagnostics],
  },
  STEAM_INTEGRATION_UNAVAILABLE: {
    title: "#L_UA_STEAM_INTEGRATION",
    text: "#L_UA_STEAM_INTEGRATION_T",
    icon: "fas fa-plug",
    actions: [ACTION.recheck, ACTION.diagnostics, ACTION.discord],
  },
  SOURCE_SDK_2013_MISSING: {
    title: "#L_UA_SDK_MISSING",
    text: "#L_UA_SDK_MISSING_T",
    icon: "fas fa-cube",
    actions: [ACTION.installSdk, ACTION.recheck],
  },
  REQUIRED_LICENSE_MISSING: {
    title: "#L_UA_LICENSE",
    text: "#L_UA_LICENSE_T",
    icon: "fas fa-id-card",
    actions: [ACTION.openSteam, ACTION.recheck, ACTION.discord],
  },
  LICENSE_CHECK_UNAVAILABLE: {
    title: "#L_UA_LICENSE_CHECK",
    text: "#L_UA_LICENSE_CHECK_T",
    icon: "fas fa-question-circle",
    actions: [ACTION.recheck, ACTION.diagnostics],
  },
  GAME_NOT_INSTALLED: {
    title: "#L_UA_NOT_INSTALLED",
    text: "#L_UA_NOT_INSTALLED_T",
    icon: "fas fa-download",
    actions: [ACTION.recheck],
  },
  DELIVERY_CHANNEL_DISABLED: {
    title: "#L_UA_CHANNEL",
    text: "#L_UA_CHANNEL_T",
    icon: "fas fa-satellite-dish",
    actions: [ACTION.recheck, ACTION.discord],
  },
  MANIFEST_UNAVAILABLE: {
    title: "#L_UA_MANIFEST",
    text: "#L_UA_MANIFEST_T",
    icon: "fas fa-wifi",
    actions: [ACTION.recheck, ACTION.diagnostics],
  },
  MANIFEST_SIGNATURE_INVALID: {
    title: "#L_UA_SIGNATURE",
    text: "#L_UA_SIGNATURE_T",
    icon: "fas fa-shield-alt",
    actions: [ACTION.recheck, ACTION.discord],
  },
  UPDATE_REQUIRED: {
    title: "#L_UA_UPDATE_REQUIRED",
    text: "#L_UA_UPDATE_REQUIRED_T",
    icon: "fas fa-sync-alt",
    actions: [ACTION.recheck],
  },
  GAME_FILES_DAMAGED: {
    title: "#L_UA_DAMAGED",
    text: "#L_UA_DAMAGED_T",
    icon: "fas fa-wrench",
    actions: [ACTION.repair],
  },
  LIBRARY_UNAVAILABLE: {
    title: "#L_UA_LIBRARY",
    text: "#L_UA_LIBRARY_T",
    icon: "fas fa-folder-minus",
    actions: [ACTION.library, ACTION.recheck],
  },
  LIBRARY_READ_ONLY: {
    title: "#L_UA_LIBRARY_RO",
    text: "#L_UA_LIBRARY_RO_T",
    icon: "fas fa-lock",
    actions: [ACTION.library, ACTION.recheck],
  },
  INSUFFICIENT_DISK_SPACE: {
    title: "#L_UA_SPACE",
    text: "#L_UA_SPACE_T",
    icon: "fas fa-hdd",
    actions: [ACTION.library, ACTION.recheck],
  },
  GAME_PROCESS_ALREADY_RUNNING: {
    title: "#L_UA_RUNNING",
    text: "#L_UA_RUNNING_T",
    icon: "fas fa-gamepad",
    actions: [ACTION.recheck],
  },
  UNKNOWN_UNAVAILABLE: {
    title: "#L_UA_UNKNOWN",
    text: "#L_UA_UNKNOWN_T",
    icon: "fas fa-info-circle",
    actions: [ACTION.recheck, ACTION.diagnostics, ACTION.discord],
  },
  BUILD_METADATA_MISSING: {
    title: "#L_UA_BUILD_META",
    text: "#L_UA_BUILD_META_MISSING_T",
    icon: "fas fa-file-alt",
    actions: [ACTION.repair, ACTION.diagnostics],
  },
  BUILD_METADATA_INVALID: {
    title: "#L_UA_BUILD_META",
    text: "#L_UA_BUILD_META_INVALID_T",
    icon: "fas fa-file-alt",
    actions: [ACTION.repair, ACTION.diagnostics],
  },
  BUILD_METADATA_MISMATCH: {
    title: "#L_UA_BUILD_META",
    text: "#L_UA_BUILD_META_MISMATCH_T",
    icon: "fas fa-file-alt",
    actions: [ACTION.repair, ACTION.diagnostics],
  },
  BUILD_METADATA_UNSUPPORTED: {
    title: "#L_UA_BUILD_META_UNSUPPORTED",
    text: "#L_UA_BUILD_META_UNSUPPORTED_T",
    icon: "fas fa-file-alt",
    actions: [ACTION.updateLauncher, ACTION.diagnostics, ACTION.discord],
  },
  BUILD_REQUIRES_NEWER_LAUNCHER: {
    title: "#L_UA_BUILD_NEWER_LAUNCHER",
    text: "#L_UA_BUILD_NEWER_LAUNCHER_T",
    icon: "fas fa-arrow-alt-circle-up",
    actions: [ACTION.updateLauncher, ACTION.diagnostics, ACTION.discord],
  },
};

export default {
  name: "UnavailableModal",
  emits: ["action", "close"],
  props: {
    reason: {
      type: String,
      required: true,
    },
    fileRepair: {
      type: Boolean,
      default: false,
    },
  },
  computed: {
    view() {
      return VIEWS[this.reason] || VIEWS.UNKNOWN_UNAVAILABLE;
    },
    // Проверка и ремонт по файлам есть не у всех игр (manifests.js, FILE_REPAIR_GAMES).
    actions() {
      if (this.fileRepair) return this.view.actions;
      const left = this.view.actions.filter((action) => action !== ACTION.repair && action !== ACTION.verify);
      return left.length ? left : [ACTION.diagnostics, ACTION.discord];
    },
  },
  methods: {
    onKeydown(e) {
      if (e.key === "Escape") {
        if (uiState.openDropdowns > 0) return;
        e.preventDefault();
        this.$emit("close");
      } else if (e.key === "Tab") {
        trapFocus(this.$refs.window, e);
      }
    },
  },
  mounted() {
    document.addEventListener("keydown", this.onKeydown);
    this.$nextTick(() => {
      let btn = this.$refs.window.querySelector("button");
      if (btn) btn.focus();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
  },
};
</script>

<style lang="scss" scoped>
.settings-modal {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;

  &__window {
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border: 1px solid var(--l-modal-border);
    border-radius: var(--l-radius);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }

  &__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 18px 16px 24px;
    border-bottom: 1px solid var(--l-border);
  }

  &__title {
    color: var(--l-text);
    font-size: 15px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.09em;
  }

  &__close {
    width: 36px;
    height: 36px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--l-text-dim);
    font-size: 17px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      color: #fff;
    }
  }
}

.unavail {
  &__window {
    width: min(520px, calc(100% - 80px));
  }

  &__body {
    padding: 20px 24px 22px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  &__head {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }

  &__icon {
    margin-top: 3px;
    width: 40px;
    min-width: 40px;
    height: 40px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--l-border-strong);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--l-text-dim);
    font-size: 17px;
  }

  &__texts {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  &__reason {
    color: var(--l-text);
    font-size: 15px;
    font-weight: 600;
    line-height: 1.35;
  }

  &__explain {
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.5;
  }

  &__details {
    summary {
      color: var(--l-text-faint);
      font-size: 12px;
      cursor: pointer;

      &:hover {
        color: var(--l-text-dim);
      }
    }

    code {
      display: inline-block;
      margin-top: 6px;
      padding: 4px 10px;
      background: rgba(255, 255, 255, 0.05);
      border: 1px solid var(--l-border);
      border-radius: 6px;
      color: var(--l-text-dim);
      font-family: Consolas, monospace;
      font-size: 12px;
    }
  }

  &__actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }
}

.settings-btn {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 9px 16px;
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--l-fast);

  i {
    color: var(--l-text-dim);
  }

  &:hover {
    background: rgba(255, 255, 255, 0.1);
  }
}
</style>
