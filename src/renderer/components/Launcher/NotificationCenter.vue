<template>
  <div class="notif" ref="root">
    <button
      type="button"
      ref="toggle"
      class="notif__btn"
      :class="{ open: open, receiving: returning, announcing: !!preview }"
      :title="$t('#L_N_TITLE')"
      :aria-label="bellAriaLabel"
      aria-haspopup="true"
      :aria-expanded="open ? 'true' : 'false'"
      @click="toggle"
    >
      <i class="fas fa-bell" aria-hidden="true"></i>
      <span v-if="unread > 0" class="notif__badge" aria-hidden="true">{{
        unread > 99 ? "99+" : unread
      }}</span>
    </button>

    <transition name="notif-flow" @after-leave="afterPreviewLeave">
      <div v-if="preview && !open && !suspended" ref="preview" class="notif__peek" :class="preview.level"
        role="status" @mouseenter="pausePreview" @mouseleave="resumePreview" @focusin="pausePreview" @focusout="resumePreview">
        <div class="notif__peek-content">
          <span class="notif__plate notif__peek-icon" aria-hidden="true"><i :class="iconOf(preview.level)"></i></span>
          <div class="notif__peek-body" @click="show">
            <button type="button" class="notif__peek-title" @click="show">{{ previewTitleOf(preview) }}</button>
            <p v-if="previewDetailOf(preview)" class="notif__peek-detail">{{ previewDetailOf(preview) }}</p>
            <div v-if="isUpdateAvailable(preview)" class="notif__peek-actions">
              <button type="button" class="notif__peek-update" @click.stop="runUpdate(preview)">
                <i class="fas fa-arrow-circle-down" aria-hidden="true"></i>{{ $t('#L_N_UPDATE_NOW') }}
              </button>
            </div>
          </div>
          <button type="button" class="notif__action notif__peek-close" :aria-label="$t('#L_CLOSE')" @click="dismissPreview">
            <i class="fas fa-times" aria-hidden="true"></i>
          </button>
        </div>
      </div>
    </transition>

    <transition name="notif-flow">
      <div v-if="open" class="notif__panel" role="dialog" :aria-label="$t('#L_N_TITLE')">
        <div class="notif__head">
          <span class="notif__plate notif__head-icon" aria-hidden="true"><i class="fas fa-bell"></i></span>
          <span class="notif__title">{{ $t("#L_N_TITLE") }}</span>
          <span class="notif__head-actions">
            <button
              v-if="items.length"
              type="button"
              class="notif__action"
              :title="$t('#L_N_MARK_ALL')"
              :aria-label="$t('#L_N_MARK_ALL')"
              @click="markAllRead"
            >
              <i class="fas fa-check-double" aria-hidden="true"></i>
            </button>
            <button
              v-if="items.length"
              type="button"
              class="notif__action"
              :title="$t('#L_N_CLEAR')"
              :aria-label="$t('#L_N_CLEAR')"
              @click="clearAll"
            >
              <i class="fas fa-trash-alt" aria-hidden="true"></i>
            </button>
          </span>
        </div>

        <div v-if="!items.length" class="notif__empty">
          <span class="notif__plate notif__empty-icon" aria-hidden="true"><i class="far fa-bell-slash"></i></span>
          <span v-grid-width>{{ $t("#L_N_EMPTY") }}</span>
        </div>

        <div v-else class="notif__list" role="list">
          <button
            v-for="(item, index) in items"
            :key="item.id"
            type="button"
            role="listitem"
            class="notif__item"
            :class="[item.level, { unread: !item.read }]"
            :style="{ '--i': Math.min(index, 8) }"
            @click="markRead(item)"
          >
            <span class="notif__plate notif__icon" aria-hidden="true"><i :class="iconOf(item.level)"></i></span>
            <span class="notif__body">
              <span class="notif__item-title">
                {{ titleOf(item) }}
                <span v-if="item.count > 1" class="notif__count">×{{ item.count }}</span>
              </span>
              <span v-if="detailOf(item)" class="notif__detail">{{ detailOf(item) }}</span>
              <span class="notif__meta">
                <span class="notif__time" v-grid-width>{{ timeOf(item.ts) }}</span>
                <span class="notif__code">{{ item.code }}</span>
              </span>


              <span v-if="isVersionCode(item)" class="notif__crash-actions">
                <button
                  v-if="item.meta && item.meta.hasReleaseNotes"
                  type="button"
                  class="notif__crash-btn" v-grid-width
                  @click.stop="whatsNew(item)"
                >
                  <i class="fas fa-list-ul" aria-hidden="true"></i>
                  {{ $t("#L_N_WHATS_NEW") }}
                </button>
                <button
                  v-if="isUpdateAvailable(item)"
                  type="button"
                  class="notif__crash-btn notif__update-btn" v-grid-width
                  @click.stop="runUpdate(item)"
                >
                  <i class="fas fa-arrow-circle-down" aria-hidden="true"></i>
                  {{ $t("#L_N_UPDATE_NOW") }}
                </button>
              </span>


              <span v-if="item.code === 'GAME_CRASHED'" class="notif__crash-actions">
                <button
                  v-if="item.meta && item.meta.hasDump"
                  type="button"
                  class="notif__crash-btn" v-grid-width
                  @click.stop="$emit('open-dump', item.meta.gameId || null)"
                >
                  <i class="fas fa-folder-open" aria-hidden="true"></i>
                  {{ $t("#L_CR_OPEN_FOLDER") }}
                </button>
                <button
                  type="button"
                  class="notif__crash-btn" v-grid-width
                  @click.stop="$emit('export-diagnostics')"
                >
                  <i class="fas fa-file-export" aria-hidden="true"></i>
                  {{ $t("#L_CR_EXPORT") }}
                </button>
                <button
                  type="button"
                  class="notif__crash-btn" v-grid-width
                  @click.stop="$emit('open-discord')"
                >
                  <i class="fab fa-discord" aria-hidden="true"></i>
                  Discord
                </button>
                <button
                  type="button"
                  class="notif__crash-btn" v-grid-width
                  @click.stop="copyCode(item)"
                >
                  <i class="fas fa-copy" aria-hidden="true"></i>
                  {{ $t("#L_CR_COPY") }}
                </button>
              </span>
            </span>
            <span v-if="!item.read" class="notif__dot" aria-hidden="true"></span>
          </button>
        </div>
      </div>
    </transition>
  </div>
</template>

<script>
import uiState from "./ui-state";
import launcherData from "./launcher-data";
import gridWidth from "./grid-width";
import LocalizationHelper from "@/utils/locales";

const { ipcRenderer } = window.hlsrElectron;
const { isProxyError } = require("../../../shared/proxy-error");
// Подсказка про прокси: путь к настройке есть только у Windows.
const IS_WINDOWS = window.hlsrElectron.platform === "win32";

const TITLE_KEYS = {
  STEAM_NOT_INSTALLED: "#L_S_LAUNCH_STEAM_NOT_INSTALLED",
  STEAM_NOT_RUNNING: "#L_S_LAUNCH_STEAM_NOT_RUNNING",
  STEAM_PROCESS_MISMATCH: "#L_S_LAUNCH_STEAM_NOT_RUNNING",
  SOURCE_SDK_NOT_INSTALLED: "#L_S_LAUNCH_SDK_MISSING",
  SOURCE_SDK_INCOMPLETE: "#L_S_LAUNCH_SDK_MISSING",
  LU_UPDATE_CONFIG_MISSING: "#L_N_UPDATE_CONFIG",
  PROFILE_IMPORTED: "#L_N_PROFILE_IMPORTED",
  GAME_CRASHED: "#L_N_GAME_CRASHED",
  LAUNCH_FAILED: "#L_N_LAUNCH_FAILED",
  LAUNCH_CONFIG_INVALID: "#L_N_LAUNCH_FAILED",
  GAME_EXECUTABLE_MISSING: "#L_N_GAME_FILES",
  GAME_DIRECTORY_MISSING: "#L_N_GAME_FILES",
  LIBRARY_PATH_CASE_CONFLICT: "#L_C_CASE_CONFLICT_TITLE",
  STEAM_APP_ID_MISMATCH: "#L_N_GAME_FILES",
  FILES_MANIFEST_INVALID: "#L_N_GAME_FILES",
  SIGNATURE_INVALID: "#L_N_UPDATE_REJECTED",
  CHECKSUM_MISMATCH: "#L_D_ERR_CHECKSUM_MISMATCH",
  ARCHIVE_INVALID: "#L_N_DELIVERY",
  ARCHIVE_UNSUPPORTED: "#L_N_DELIVERY",
  PATCH_INVALID: "#L_N_DELIVERY",
  PATCH_FAILED: "#L_N_DELIVERY",
  PATCH_CHAIN_MISSING: "#L_D_ERR_PATCH_CHAIN_MISSING",
  INSTALL_FAILED: "#L_N_DELIVERY",
  PRESERVE_FAILED: "#L_N_DELIVERY",
  ANTIVIRUS_BLOCKED: "#L_N_ANTIVIRUS",
  UPDATE_FAILED: "#L_N_DELIVERY",
  REPAIR_FAILED: "#L_N_REPAIR",
  REPAIR_SOURCE_MISSING: "#L_D_ERR_REPAIR_SOURCE_MISSING",
  NO_SPACE: "#L_D_ERR_NO_SPACE",
  PATH_NOT_WRITABLE: "#L_N_LIBRARY",
  PATH_INVALID: "#L_N_LIBRARY",
  FILE_LOCKED: "#L_D_ERR_FILE_LOCKED",
  GAME_RUNNING: "#L_D_ERR_GAME_RUNNING",
  // Операция с игрой во время экспорта клипа из демки (инструмент «Демо»).
  RENDER_ACTIVE: "#L_D_ERR_RENDER_ACTIVE",
  MANIFEST_UNAVAILABLE: "#L_N_FEED_OFFLINE",
  NETWORK_ERROR: "#L_N_FEED_OFFLINE",
  FEED_UNAVAILABLE: "#L_N_FEED_OFFLINE",
  LU_FEED_UNAVAILABLE: "#L_N_FEED_OFFLINE",
  LU_FEED_STALE: "#L_N_FEED_STALE",
  UPDATE_CHECK_TIMEOUT: "#L_N_FEED_OFFLINE",
  LU_SIGNATURE_INVALID: "#L_N_UPDATE_REJECTED",
  LU_SCHEMA_INVALID: "#L_N_UPDATE_REJECTED",
  LU_INSTALLER_SHA256: "#L_N_UPDATE_REJECTED",
  LU_INSTALLER_SHA512: "#L_N_UPDATE_REJECTED",
  LU_INSTALLER_SIZE: "#L_N_UPDATE_REJECTED",
  LU_VERSION_MISMATCH: "#L_N_UPDATE_REJECTED",
  LU_VERSION_UNSUPPORTED: "#L_N_UPDATE_REJECTED",
  LU_LATEST_YML_MISMATCH: "#L_N_UPDATE_REJECTED",
  LAUNCHER_UPDATE_ERROR: "#L_N_UPDATE_LAUNCHER",
  STARTUP_ERROR: "#L_N_STARTUP_ERROR",
  LIBRARY_LEGACY_LAYOUT: "#L_N_LEGACY_LIBRARY",
  PROTON_UPDATED: "#L_N_PROTON_UPDATED",
  PROTON_INSTALLING: "#L_N_PROTON_INSTALLING",
  PROTON_INSTALLED: "#L_N_PROTON_INSTALLED",
  PROTON_INSTALL_FAILED: "#L_S_PROTON_ERROR",
  PROTON_UPDATE_AVAILABLE: "#L_N_PROTON_UPDATE_AVAILABLE",

  LAUNCHER_UPDATE_AVAILABLE: "#L_N_V_LAUNCHER_AVAILABLE",
  LAUNCHER_UPDATED: "#L_N_V_LAUNCHER_UPDATED",
  GAME_UPDATE_AVAILABLE: "#L_N_V_GAME_AVAILABLE",
  GAME_INSTALLED: "#L_N_V_GAME_INSTALLED",
  GAME_UPDATED: "#L_N_V_GAME_UPDATED",

  CLIENT_MODIFIED: "#L_N_CLIENT_MODIFIED",
  SUPPORT_THANKS: "#L_N_SUPPORT_THANKS",
  MOD_BAN: "#L_N_MOD_BAN",
  MOD_UNBAN: "#L_N_MOD_UNBAN",
  MOD_MUTE: "#L_N_MOD_MUTE",
  MOD_UNMUTE: "#L_N_MOD_UNMUTE",
  MOD_RESET_STATUS: "#L_N_MOD_RESET_STATUS",
  MOD_RESET_NICK: "#L_N_MOD_RESET_NICK",
  MOD_RESET_AVATAR: "#L_N_MOD_RESET_AVATAR",
  MOD_RESTORE_NICK: "#L_N_MOD_RESTORE_NICK",
  MOD_RESTORE_AVATAR: "#L_N_MOD_RESTORE_AVATAR",
};

const VERSION_CODES = new Set([
  "PROTON_UPDATE_AVAILABLE",
  "LAUNCHER_UPDATE_AVAILABLE",
  "LAUNCHER_UPDATED",
  "GAME_UPDATE_AVAILABLE",
  "GAME_INSTALLED",
  "GAME_UPDATED",
]);

const VERSIONED_TITLE_CODES = new Set(["LAUNCHER_UPDATED", "GAME_UPDATED"]);

const DEP_HINTS = {
  DEP_ALWAYS_ON: "#L_CR_DEP_ALWAYS_ON",
  DEP_EXEMPTION_FAILED: "#L_CR_DEP_EXEMPTION_FAILED",
};
const GAME_TITLE_CODES = new Set(["GAME_UPDATE_AVAILABLE", "GAME_INSTALLED", "GAME_UPDATED", "GAME_CRASHED"]);

export default {
  name: "NotificationCenter",
  directives: { gridWidth },
  emits: ["export-diagnostics", "open-discord", "open-dump", "run-update", "whats-new"],
  props: { suspended: { type: Boolean, default: false } },
  data() {
    return {
      open: false,
      items: [],
      unread: 0,
      preview: null,
      previewQueue: [],
      seenItems: {},
      hydrated: false,
      refreshSequence: 0,
      disposed: false,
      previewTimer: null,
      previewRemaining: 4800,
      previewStartedAt: 0,
      returning: false,
    };
  },
  computed: {
    bellAriaLabel() {
      let title = this.$t("#L_N_TITLE");
      return this.unread > 0 ? `${title} (${this.unread})` : title;
    },
    uiLanguage() {
      try {
        return Object.keys(LocalizationHelper.locales)[LocalizationHelper.lang] === "en"
          ? "en"
          : "ru";
      } catch (e) {
        return "ru";
      }
    },
  },
  watch: {
    suspended(value) {
      if (value && this.preview) {
        this.previewQueue.unshift(this.preview);
        this.dismissPreview();
      } else if (!value) this.nextPreview();
    },
  },
  methods: {
    async refresh() {
      const sequence = ++this.refreshSequence;
      try {
        const result = await ipcRenderer.invoke("notifications-list");
        if (!result || this.disposed || sequence !== this.refreshSequence) return;
        this.applySnapshot(result);
      } catch (error) { /* сохраняем уже показанные уведомления */ }
    },
    applySnapshot(result) {
      const items = result.items || [];
      const fresh = items.filter((item) => !item.read && (this.hydrated
        ? !this.seenItems[item.id] || (item.count || 1) > this.seenItems[item.id]
        : this.isUpdateAvailable(item) || Date.now() - item.ts < 15000));
      this.items = items;
      this.unread = result.unread || 0;
      // «Синдром Плюшкина»: в списке накопилось 50 уведомлений.
      if (items.length >= 50) ipcRenderer.invoke("runner-secret-unlock", "hoarder").catch(() => {});
      for (const item of items) this.seenItems[item.id] = item.count || 1;
      this.hydrated = true;
      this.previewQueue = this.previewQueue.filter((queued) => items.some((item) => item.id === queued.id && !item.read));
      if (this.preview && !items.some((item) => item.id === this.preview.id && !item.read)) this.dismissPreview();
      if (!this.open) {
        for (const item of fresh.slice(0, 5).reverse()) {
          if (this.preview && this.preview.id === item.id) {
            this.preview = item;
            continue;
          }
          const queued = this.previewQueue.findIndex((entry) => entry.id === item.id);
          if (queued >= 0) this.previewQueue.splice(queued, 1, item);
          else this.previewQueue.push(item);
        }
      }
      this.nextPreview();
    },
    nextPreview() {
      if (this.preview || this.returning || this.open || this.suspended || this.disposed || !this.previewQueue.length) return;
      this.preview = this.previewQueue.shift();
      this.previewRemaining = 4800;
      this.resumePreview();
    },
    pausePreview() {
      if (!this.previewTimer) return;
      this.previewRemaining = Math.max(0, this.previewRemaining - (Date.now() - this.previewStartedAt));
      clearTimeout(this.previewTimer);
      this.previewTimer = null;
    },
    resumePreview(event) {
      if (!this.preview || this.previewTimer || this.disposed) return;
      const element = this.$refs.preview;
      if (element && (element.matches(":hover") || element.contains(event && event.relatedTarget || document.activeElement))) return;
      this.previewStartedAt = Date.now();
      this.previewTimer = setTimeout(() => this.dismissPreview(), this.previewRemaining);
    },
    dismissPreview() {
      clearTimeout(this.previewTimer);
      this.previewTimer = null;
      if (!this.preview) return;
      this.returning = !!(this.$refs.preview && this.$refs.preview.isConnected);
      this.preview = null;
      if (!this.returning) this.$nextTick(() => this.nextPreview());
    },
    afterPreviewLeave() {
      this.returning = false;
      this.nextPreview();
    },
    onChanged() {
      this.refresh();
    },
    toggle() {
      this.open ? this.close() : this.show();
    },
    show() {
      if (this.open) return;
      this.previewQueue = [];
      this.dismissPreview();
      this.open = true;
      uiState.openDropdowns++;
      this.refresh();
      document.addEventListener("mousedown", this.onDocumentDown, true);
      document.addEventListener("keydown", this.onDocumentKey, true);
    },
    close(focusToggle) {
      if (!this.open) return;
      this.open = false;
      uiState.openDropdowns--;
      document.removeEventListener("mousedown", this.onDocumentDown, true);
      document.removeEventListener("keydown", this.onDocumentKey, true);
      if (focusToggle && this.$refs.toggle) this.$refs.toggle.focus();
      this.nextPreview();
    },
    onDocumentDown(e) {
      if (!this.$refs.root.contains(e.target)) this.close();
    },
    onDocumentKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close(true);
      }
    },
    async markRead(item) {
      if (item.read) return;
      await ipcRenderer.invoke("notifications-mark-read", item.id);
      this.refresh();
    },
    async markAllRead() {
      await ipcRenderer.invoke("notifications-mark-all-read");
      this.refresh();
    },
    async clearAll() {
      this.previewQueue = [];
      this.dismissPreview();
      await ipcRenderer.invoke("notifications-clear");
      this.refresh();
    },
    // Модерация: причина и срок. Причину пишет человек, она показывается
    // на любом языке интерфейса.
    moderationDetail(item) {
      const parts = [];
      const meta = item.meta || {};
      if (meta.until) {
        const locale = this.uiLanguage === "en" ? "en-GB" : "ru-RU";
        parts.push(this.$t("#L_N_MOD_UNTIL", new Date(meta.until).toLocaleString(locale, { day: "numeric", month: "long", year: "numeric", hour: "2-digit", minute: "2-digit" })));
      } else if (meta.forever) parts.push(this.$t("#L_N_MOD_FOREVER"));
      if (item.detail) parts.push(this.$t("#L_MOD_REASON", item.detail));
      if (item.code === "MOD_RESET_NICK") parts.push(this.$t("#L_N_MOD_NICK_HINT"));
      if (item.code === "MOD_RESET_AVATAR") parts.push(this.$t("#L_N_MOD_AVATAR_HINT"));
      return parts.join(" · ") || null;
    },
    detailOf(item) {
      if (/^MOD_/.test(item.code)) return this.moderationDetail(item);
      if (item.code === "SUPPORT_THANKS") return this.$t("#L_THANKS_TEXT");
      if (item.code === "CLIENT_MODIFIED") return this.$t("#L_N_CLIENT_MODIFIED_TEXT");
      if (item.code === "PROTON_UPDATE_AVAILABLE" && item.meta && item.meta.newVersion)
        return this.$t("#L_N_PROTON_UPDATE_TOAST", item.meta.newVersion.replace(/^wineland-/, ""));
      if (item.code === "GAME_CRASHED" && item.meta && DEP_HINTS[item.meta.hint])
        return this.$t(DEP_HINTS[item.meta.hint]);
      if (item.code === "GAME_CRASHED")
        return item.meta && item.meta.hasDump
          ? this.$t("#L_CR_TEXT_DUMP")
          : this.$t("#L_CR_TEXT_NO_DUMP");

      if (VERSION_CODES.has(item.code) && item.meta) {
        const from = item.meta.oldVersion;
        const to = item.meta.newVersion;
        if (from && to && from !== to) return `${from} → ${to}`;
        if (to) return this.$t("#L_N_VERSION", to);
      }

      if (!item.detail) return item.detail;
      if (isProxyError(item.detail))
        return this.$t(IS_WINDOWS ? "#L_N_PROXY_HINT" : "#L_N_PROXY_HINT_SYSTEM");
      if (this.uiLanguage !== "en") return item.detail;

      return /[А-Яа-яЁё]/.test(item.detail) ? null : item.detail;
    },
    previewDetailOf(item) {
      if (item.meta && item.meta.newVersion) {
        const short = (version) => String(version).replace(/^wineland-/, "");
        if (item.meta.oldVersion && item.meta.oldVersion !== item.meta.newVersion)
          return `${short(item.meta.oldVersion)} → ${short(item.meta.newVersion)}`;
        return short(item.meta.newVersion);
      }
      return this.detailOf(item);
    },
    previewTitleOf(item) {
      if (item.code === "PROTON_UPDATE_AVAILABLE") return this.$t("#L_N_UPDATE_FOR", "Proton Wineland");
      if (item.code === "LAUNCHER_UPDATE_AVAILABLE") return this.$t("#L_N_UPDATE_FOR", "HLSR");
      if (item.code === "GAME_UPDATE_AVAILABLE") return this.$t("#L_N_UPDATE_FOR", this.gameNameOf(item));
      return this.titleOf(item);
    },
    isVersionCode(item) {
      return VERSION_CODES.has(item.code);
    },
    isUpdateAvailable(item) {
      return (
        item.code === "LAUNCHER_UPDATE_AVAILABLE" || item.code === "GAME_UPDATE_AVAILABLE" ||
        item.code === "PROTON_UPDATE_AVAILABLE"
      );
    },
    whatsNew(item) {
      this.markRead(item);
      this.$emit("whats-new", {
        target: item.meta ? item.meta.target : null,
        gameId: item.meta ? item.meta.gameId : null,
        version: item.meta ? item.meta.newVersion : null,
      });
      this.close();
    },
    runUpdate(item) {
      this.dismissPreview();
      this.markRead(item);
      this.$emit("run-update", {
        target: item.meta ? item.meta.target : null,
        gameId: item.meta ? item.meta.gameId : null,
        version: item.meta ? item.meta.newVersion : null,
      });
      this.close();
    },
    copyCode(item) {
      let code = `GAME_CRASHED exit=${item.meta && item.meta.exitCode != null ? item.meta.exitCode : "?"}`;
      if (item.meta && item.meta.hint) code += ` ${item.meta.hint}`;
      try {
        navigator.clipboard.writeText(code);
      } catch (e) {
        /* clipboard может быть недоступен */
      }
    },
    iconOf(level) {
      if (level === "error") return "fas fa-times-circle";
      if (level === "warning") return "fas fa-exclamation-triangle";
      return "fas fa-info-circle";
    },
    gameNameOf(item) {
      const id = item.meta && item.meta.launcherId;
      const name = id ? launcherData.shortNames[id] : null;
      return name || this.$t("#L_N_V_GAME_FALLBACK");
    },
    titleOf(item) {
      let key = TITLE_KEYS[item.code];

      if (key) {
        const args = [];

        if (GAME_TITLE_CODES.has(item.code)) args.push(this.gameNameOf(item));

        if (VERSIONED_TITLE_CODES.has(item.code) && item.meta && item.meta.newVersion)
          args.push(item.meta.newVersion);

        let text = args.length ? this.$t(key, ...args) : this.$t(key);
        if (text && text !== key) return text;
      }

      return this.$t("#L_N_GENERIC");
    },
    timeOf(ts) {
      let date = new Date(ts);
      let now = new Date();
      let hm =
        String(date.getHours()).padStart(2, "0") +
        ":" +
        String(date.getMinutes()).padStart(2, "0");

      if (date.toDateString() === now.toDateString()) return hm;

      return (
        String(date.getDate()).padStart(2, "0") +
        "." +
        String(date.getMonth() + 1).padStart(2, "0") +
        " " +
        hm
      );
    },
  },
  mounted() {
    ipcRenderer.on("notifications-changed", this.onChanged);
    this.refresh();
  },
  beforeUnmount() {
    this.disposed = true;
    clearTimeout(this.previewTimer);
    ipcRenderer.removeListener("notifications-changed", this.onChanged);
    if (this.open) this.close();
  },
};
</script>

<style lang="scss" scoped>
@use "./round-dot" as *;

.notif {
  position: relative;
  -webkit-app-region: no-drag;

  &__btn {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--l-header-control);
    height: var(--l-header-control);
    padding: 0;
    background: transparent;
    border: none;
    border-radius: var(--l-radius-sm);
    color: var(--l-text-dim);
    font-size: 24px;
    cursor: pointer;
    transition: background var(--l-fast), border-color var(--l-fast),
      color var(--l-fast);

    > i {
      display: block;
      width: 24px;
      line-height: 24px;
      text-align: center;
    }

    &:hover,
    &.open {
      color: var(--l-text);
    }

    &:active {
      color: var(--accent);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: 0;
    }
  }

  /* Рамка не занимает места; счётчик отступает от края ровно на 4 px. */
  &__badge {
    position: absolute;
    top: -4px;
    /* У правого верхнего угла значка, который стоит по центру кнопки. */
    right: calc(var(--l-header-inset, 8px) - 4px);
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    background: var(--accent);
    border-radius: 8px;
    color: var(--accent-contrast-text);
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
    text-align: center;
    pointer-events: none;
  }

  /* Окно — как меню аккаунта и окно профиля: непрозрачная поверхность,
     рамка тенью (border сдвинул бы всё содержимое с сетки на пиксель),
     шапка и пункты — карточками. */
  &__panel {
    position: absolute;
    top: calc(100% + 8px);
    /* Колокольчик стоит у правого края окна: панель раскрывается влево */
    right: 0;
    width: 372px;
    max-height: min(460px, calc(100vh - 120px));
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    background: var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
    overflow: hidden;
    z-index: 40;
  }

  &__panel, &__peek {
    transform-origin: calc(100% - 12px) calc(var(--l-header-control) * -0.5 - 8px);
    -webkit-app-region: no-drag;
  }

  /* Значок в плашке цвета уведомления, как у тостов. Цвет задаёт
     --notif-tint у пункта или всплывающей карточки. */
  &__plate {
    flex: none;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--l-radius-sm);
    background: color-mix(in srgb, var(--notif-tint, var(--l-accent)) 16%, transparent);
    color: var(--notif-tint, var(--l-accent));
    font-size: 14px;

    > i { line-height: 1; }
  }

  &__item, &__peek {
    --notif-tint: var(--l-accent);

    &.warning { --notif-tint: #ffb37a; }
    &.error { --notif-tint: #ff7a7a; }
  }

  /* Всплывающая карточка — как тост: подсветка цветом уведомления из угла
     и рамка того же оттенка. */
  &__peek {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    width: 344px;
    background:
      radial-gradient(120% 140% at 0% 0%, color-mix(in srgb, var(--notif-tint) 12%, transparent) 0%, transparent 60%),
      var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: var(--l-shadow-pop);
    overflow: hidden;
    z-index: 41;
  }
  &__peek-content { display: flex; align-items: flex-start; gap: 12px; padding: 12px; }
  &__peek-body { flex: 1; min-width: 0; padding-top: 6px; cursor: pointer; }
  &__peek-title {
    padding: 0; border: none; background: transparent; color: var(--l-text);
    font-family: inherit; font-size: 13px; font-weight: 600; line-height: 20px; text-align: left; cursor: pointer;
  }
  &__peek-detail {
    margin: 4px 0 0; color: var(--l-text-dim); font-size: 12px; line-height: 16px; overflow-wrap: anywhere;
    display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;
  }
  &__peek-actions { margin-top: 8px; }
  &__peek-update {
    display: inline-flex; align-items: center; gap: 6px; padding: 0; border: none;
    background: transparent; color: var(--accent); font-family: inherit; font-size: 12px; line-height: 20px;
    cursor: pointer;
    &:hover { color: var(--accent-hover); }
  }
  &__peek-close { flex: none; width: 24px; height: 24px; margin: 4px -4px 0 0; }
  &__peek-title:focus-visible, &__peek-actions button:focus-visible, &__peek-close:focus-visible {
    outline: 2px solid var(--accent-focus); outline-offset: 2px;
  }
  &__btn.announcing { color: var(--l-text); }
  &__btn.receiving > i { animation: notif-receive var(--l-dur-slow) var(--l-ease); }

  /* Шапка без рамки. */
  &__head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px;
    border-radius: var(--l-radius-sm);
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
  }

  &__head-icon {
    background: rgba(255, 255, 255, 0.04);
    color: var(--l-text-dim);
  }

  &__title {
    flex: 1;
    min-width: 0;
    color: var(--l-text);
    font-size: 15px;
    font-weight: 700;
    line-height: 20px;
  }

  &__head-actions {
    display: flex;
    gap: 4px;
  }

  &__action {
    width: 32px;
    height: 32px;
    padding: 0;
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 16px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);

    > i {
      width: 16px;
      line-height: 16px;
    }

    &:hover {
      background: var(--l-accent-soft);
      color: var(--l-text);
    }
  }

  &__empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 24px;
    color: var(--l-text-faint);
    font-size: 13px;
    line-height: 20px;
    animation: l-rise-in var(--l-dur) var(--l-ease) 40ms both;
  }

  &__empty-icon {
    width: 48px;
    height: 48px;
    background: rgba(255, 255, 255, 0.04);
    color: var(--l-text-faint);
    font-size: 20px;
  }

  &__list {
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* Пункт без рамки и без фона: непрочитанный отмечен только точкой,
     при наведении фон светлеет. Пункты поднимаются по
     очереди при открытии окна. */
  &__item {
    flex: none;
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 12px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color var(--l-fast);
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
    animation-delay: calc(40ms + var(--i, 0) * 30ms);

    &:hover {
      background-color: rgba(255, 255, 255, 0.05);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: -2px;
    }
  }

  /* Строка заголовка (20) по центру плашки (32). */
  &__body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-top: 6px;
  }

  &__item-title {
    color: var(--l-text);
    font-size: 13px;
    font-weight: 500;
    line-height: 20px;
  }

  /* line-height 1: мелкий шрифт на базовой линии строки 20 раздувал её
     до 21 px, и всё ниже в пункте уходило с сетки */
  &__count {
    line-height: 1;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 600;
  }

  &__detail {
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 16px;
    word-break: break-word;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  &__meta {
    display: flex;
    align-items: center;
    gap: 8px;
    line-height: 16px;
  }

  &__time {
    color: var(--l-text-faint);
    font-size: 11px;
  }

  &__code {
    color: var(--l-text-faint);
    font-family: Consolas, monospace;
    font-size: 10.5px;
    letter-spacing: 0.02em;
  }

  &__dot {
    flex: none;
    margin-top: 12px;
    width: 8px;
    height: 8px;
    @include round-dot(var(--accent));
  }

  &__crash-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 4px 0 0;
  }

  /* Рамка — тенью внутрь: высота кнопки ровно 24 */
  &__crash-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 24px;
    padding: 0 12px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px var(--l-border-strong);
    color: var(--l-text);
    font-family: inherit;
    font-size: 11.5px;
    line-height: 16px;
    cursor: pointer;
    transition: background var(--l-fast), box-shadow var(--l-fast);

    i {
      width: 12px;
      color: var(--l-text-dim);
      font-size: 11px;
      line-height: 16px;
      text-align: center;
    }

    &:hover {
      background: rgba(255, 255, 255, 0.1);
    }
  }

  &__update-btn {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1px var(--accent-border);

    i {
      color: var(--accent);
    }

    &:hover {
      background: var(--accent-soft);
      box-shadow: inset 0 0 0 1px var(--accent);
    }
  }
}

.notif-flow-enter-active,
.notif-flow-leave-active {
  transition: transform var(--l-dur-slow) var(--l-ease), opacity var(--l-dur) var(--l-ease), filter var(--l-dur-slow) var(--l-ease), border-radius var(--l-dur-slow) var(--l-ease);
}
.notif-flow-leave-active {
  pointer-events: none;
}
.notif-flow-enter-from, .notif-flow-leave-to {
  transform: scale(0.08);
  opacity: 0;
  filter: blur(6px);
  border-radius: 48px;
}
@keyframes notif-receive {
  0%, 65%, 100% { transform: scale(1); }
  82% { transform: scale(1.12); color: var(--accent); }
}
</style>
