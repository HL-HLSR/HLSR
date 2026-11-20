<template>
  <footer class="download-bar">

    <transition name="db-status">
      <div v-if="showStatus" class="download-bar__status">
        <span class="download-bar__icon">
          <i class="fas fa-arrow-down"></i>
        </span>
        <div class="download-bar__info">
          <span class="download-bar__label">{{ statusText }}</span>
          <div
            class="download-bar__track"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="percent"
          >
            <div
              class="download-bar__fill"
              :class="{ done: !isDownloading }"
              :style="{ width: percent + '%' }"
            ></div>
          </div>
          <div class="download-bar__meta">
            <span>{{ doneText }} / {{ totalText }}</span>
            <span>{{ rightMeta }}</span>
          </div>
        </div>
      </div>
    </transition>

    <div class="download-bar__controls">
      <button v-if="canCancel" type="button" class="download-bar__cancel"
        :title="$t('#L_CANCEL')" :aria-label="$t('#L_CANCEL')" @click="$emit('cancel', pausedEpisode)">
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>
      <BuildSelector
        class="download-bar__build"
        :builds="game.demo.builds"
        :upcoming="game.demo.upcoming || []"
        :badges="buildBadges"
        :value="buildValue"
        @select-build="$emit('select-build', $event)"
      />

      <GameMenu
        :gameId="deliveryGameId"
        :episode="menuEpisode"
        @action="$emit('menu-action', $event)"
        @open="$emit('menu-open')"
      />


      <button
        type="button"
        class="download-bar__play"
        :class="['tone-' + buttonTone, { 'is-busy': !buttonEnabled }]"
        :disabled="!buttonEnabled"
        :title="actionText"
        :aria-label="actionText"
        @click="onAction"
      >
        <span class="download-bar__play-label" v-grid-width:8>
          <i :class="actionIcon" aria-hidden="true"></i>
          <span>{{ actionText }}</span>
        </span>
      </button>
    </div>
  </footer>
</template>

<script>
import GameMenu from "./GameMenu";
import BuildSelector from "./BuildSelector";
import gridWidth from "./grid-width";
import progress from "../../../shared/progress-states";
import { formatNumber, formatSize } from "./file-format";
const { episodeButton } = require("../../../shared/episode-button");

const DELIVERY_LABELS = {
  "not-installed": "#L_D_NOT_INSTALLED",
  preparing: "#L_D_PREPARING",
  downloading: "#L_D_DOWNLOADING",
  paused: "#L_D_PAUSED",
  verifying: "#L_D_VERIFYING",
  extracting: "#L_D_EXTRACTING",
  finalizing: "#L_D_FINALIZING",
  "applying-update": "#L_D_APPLYING",
  "checking-files": "#L_D_CHECKING",
  "files-valid": "#L_D_FILES_VALID",
  "files-damaged": "#L_D_FILES_DAMAGED",
  repairing: "#L_D_REPAIRING",
  "repair-complete": "#L_D_REPAIR_DONE",
  exporting: "#L_D_EXPORTING",
  importing: "#L_D_IMPORTING",
  ready: "#L_D_READY",
  cancelled: "#L_D_CANCELLED",
  error: "#L_D_ERROR",
};

const FINISH_HOLD_MS = 1000;

const BUTTON = {
  ready: { icon: "fas fa-play", key: "#L_B_PLAY" },
  "not-installed": { icon: "fas fa-download", key: "#L_B_INSTALL" },
  "episode-not-installed": { icon: "fas fa-download", key: "#L_BT_EPISODE_DOWNLOAD" },
  "episode-paused": { icon: "fas fa-play", key: "#L_B_RESUME" },
  unavailable: { icon: "fas fa-ban", key: "#L_B_UNAVAILABLE" },
  "checking-updates": { icon: "fas fa-circle-notch fa-spin", key: "#L_B_CHECKING_UPDATES" },
  "update-available": { icon: "fas fa-sync-alt", key: "#L_B_UPDATE" },
  "downloading-update": { icon: "fas fa-pause", key: "#L_B_PAUSE" },
  "update-paused": { icon: "fas fa-play", key: "#L_B_RESUME" },
  "verifying-update": { icon: "fas fa-circle-notch fa-spin", key: "#L_B_VERIFYING_UPDATE" },
  "applying-update": { icon: "fas fa-circle-notch fa-spin", key: "#L_B_APPLYING_UPDATE" },
  "update-error": { icon: "fas fa-redo", key: "#L_B_UPDATE_ERROR" },
  "starting-game": { icon: "fas fa-circle-notch fa-spin", key: "#L_B_STARTING_GAME" },
  "game-running": { icon: "fas fa-times", key: "#L_B_GAME_RUNNING" },
  "stopping-game": { icon: "fas fa-circle-notch fa-spin", key: "#L_B_STOPPING_GAME" },
  downloading: { icon: "fas fa-pause", key: "#L_B_PAUSE" },
  "download-paused": { icon: "fas fa-play", key: "#L_B_RESUME" },
  installing: { icon: "fas fa-circle-notch fa-spin", key: "#L_B_INSTALLING" },
  busy: { icon: "fas fa-circle-notch fa-spin", key: "#L_B_BUSY" },
  error: { icon: "fas fa-redo", key: "#L_B_RETRY" },
  "repair-required": { icon: "fas fa-wrench", key: "#L_B_REPAIR" },
  "update-launcher-required": { icon: "fas fa-arrow-up", key: "#L_B_UPDATE_LAUNCHER" },
};

const TONES = {
  ready: "ready",
  "not-installed": "progress",
  "episode-not-installed": "progress",
  downloading: "progress",
  "download-paused": "progress",
  "episode-paused": "progress",
  installing: "progress",
  busy: "progress",
  "checking-updates": "progress",
  "update-available": "progress",
  "downloading-update": "progress",
  "update-paused": "progress",
  "verifying-update": "progress",
  "applying-update": "progress",
  "repair-required": "repair",
  error: "danger",
  "update-error": "danger",
  unavailable: "muted",
  "starting-game": "neutral",
  "game-running": "neutral",
  "stopping-game": "neutral",
  "update-launcher-required": "muted",
};

const ACTIONS = ["play", "stop", "install", "update", "pause", "resume", "retry", "repair", "explain"];

export default {
  name: "DownloadBar",
  components: { GameMenu, BuildSelector },
  directives: { gridWidth },
  emits: ["cancel", "explain", "install", "install-episode", "menu-action", "menu-open", "pause", "play", "repair", "resume", "retry", "select-build", "stop", "update"],
  props: {
    game: {
      type: Object,
      required: true,
    },
    buildValue: {
      type: String,
      required: true,
    },
    delivery: {
      type: Object,
      default: null,
    },
  },
  data() {
    return {
      finishHold: false,
      finishState: null,
      finishOperation: null,
      finishTotal: 0,
      finishTimer: null,
      watched: {
        gameId: this.delivery ? this.delivery.gameId : null,
        state: this.delivery ? this.delivery.state : null,
      },
    };
  },
  computed: {
    // Эпизод на паузе: Ghosting в «ready», а докачку можно бросить — main убирает архив только этого эпизода.
    pausedEpisode() {
      return this.buttonState === "episode-paused" ? this.episodeId : null;
    },
    canCancel() {
      if (this.pausedEpisode) return true;
      return !!(this.delivery && this.delivery.source !== "steam-depot" && ["preparing", "downloading", "paused", "verifying", "extracting", "exporting", "importing"].includes(this.delivery.state));
    },
    // Выбранный в «Версии сборки» эпизод Ghosting ("ep1" | "ep2") или null.
    episodeId() {
      return (this.game.demo.episodes || {})[this.buildValue] || null;
    },
    episodesState() {
      return (this.delivery && this.delivery.episodes) || {};
    },
    // Сколько скачается для нескачанного эпизода: «2,7 ГБ». «Скачать» качает все недостающие
    // эпизоды сразу (LauncherShell.onInstallEpisode), поэтому у каждого — их общий размер;
    // без манифеста хоть одного из них — ничего.
    buildBadges() {
      const episodes = this.game.demo.episodes || {};
      const state = this.episodesState;
      const missing = Object.values(episodes).filter((id) => state[id] && !state[id].installed);
      if (!missing.length || missing.some((id) => !(state[id].size > 0))) return {};
      const size = formatSize(missing.reduce((sum, id) => sum + state[id].size, 0), this.$t);
      const badges = {};
      for (const [build, id] of Object.entries(episodes)) if (missing.includes(id)) badges[build] = size;
      return badges;
    },
    // Для меню: при выбранном эпизоде «Удалить эпизод» вместо «Удалить игру».
    menuEpisode() {
      if (!this.episodeId) return null;
      return { installed: !!(this.episodesState[this.episodeId] || {}).installed };
    },
    deliveryState() {
      return this.delivery ? this.delivery.state : null;
    },
    deliveryGameId() {
      return this.delivery ? this.delivery.gameId : null;
    },
    showStatus() {
      if (!this.delivery) return this.download.total > 0;
      if (progress.isProgress(this.delivery.state)) return true;
      if (this.delivery.state === "files-damaged") return true;
      return this.finishHold;
    },
    download() {
      return this.game.demo.download || { done: 0, total: 0 };
    },
    isDownloading() {
      if (this.delivery) return progress.isBusy(this.delivery.state);
      return this.download.done < this.download.total;
    },
    percent() {
      if (this.finishHold) return 100;
      if (this.delivery) return Math.round(this.delivery.percent || 0);
      return this.download.total > 0
        ? Math.round((this.download.done / this.download.total) * 100)
        : 0;
    },
    statusText() {
      if (!this.delivery)
        return this.isDownloading
          ? this.$t("#L_DOWNLOADING_FILES")
          : this.$t("#L_BUILD_INSTALLED");

      if (this.finishHold) {
        if (this.finishState === "files-valid") return this.$t("#L_D_FILES_VALID");
        if (this.finishState === "repair-complete") return this.$t("#L_D_REPAIR_DONE");
        if (this.finishOperation === "update") return this.$t("#L_U_G_DONE");
        if (this.finishOperation === "export") return this.$t("#L_D_EXPORT_DONE");
        if (this.finishOperation === "import") return this.$t("#L_D_IMPORT_DONE");
        return this.$t("#L_D_COMPLETED");
      }

      if (this.steamDownload) return this.$t("#L_D_DOWNLOADING_STEAM");

      if (this.isUpdating) return this.updateStatusText;

      if (this.delivery.operation === "repair-download" && this.delivery.state === "preparing")
        return this.$t("#L_DOWNLOADING_FILES");

      if (this.delivery.state === "files-damaged" && this.delivery.integrity)
        return this.$t("#L_D_FILES_DAMAGED_N", String(this.delivery.integrity.damaged));

      if (this.delivery.state === "not-installed" && !this.delivery.installable)
        return this.$t("#L_D_UNAVAILABLE");

      if (this.delivery.state === "error" && this.delivery.error)
        return this.errorText(this.delivery.error.code);

      return this.$t(DELIVERY_LABELS[this.delivery.state] || "#L_D_NOT_INSTALLED");
    },
    isUpdating() {
      return !!(this.delivery && this.delivery.operation === "update");
    },
    chain() {
      return this.delivery ? this.delivery.chain : null;
    },
    updateStatusText() {
      const chain = this.chain;

      if (chain && chain.steps > 1 && this.delivery.state === "downloading")
        return this.$t("#L_U_G_CHAIN", String(chain.step), String(chain.steps));

      switch (this.delivery.state) {
        case "downloading":
        case "paused":
          return this.$t("#L_U_G_DOWNLOADING", chain ? chain.toVersion : "");
        case "preparing":
        case "verifying":
          return this.$t("#L_U_G_VERIFYING");
        default:
          return this.$t("#L_U_G_APPLYING");
      }
    },
    countsFiles() {
      return (
        this.delivery &&
        ["checking-files", "repairing", "files-damaged"].indexOf(this.delivery.state) !== -1
      );
    },
    steamDownload() {
      return !!(
        this.delivery &&
        this.delivery.source === "steam-depot" &&
        ["preparing", "downloading"].indexOf(this.delivery.state) !== -1
      );
    },
    doneText() {
      if (!this.delivery) return this.sizeText(this.download.done);
      if (this.steamDownload) return String(this.delivery.filesDone || 0);
      if (this.countsFiles) return String(this.delivery.checked || 0);
      if (this.finishHold) return this.bytesText(this.progressTotalBytes);

      if (this.chain)
        return this.bytesText((this.chain.doneBytes || 0) + (this.delivery.bytesDownloaded || 0));

      return this.bytesText(this.delivery.bytesDownloaded);
    },
    totalText() {
      if (!this.delivery) return this.sizeText(this.download.total);
      if (this.steamDownload)
        return `${this.delivery.filesTotal || 0} ${this.$t("#L_D_FILES_UNIT")}`;
      if (this.countsFiles)
        return `${this.delivery.totalChecked || 0} ${this.$t("#L_D_FILES_UNIT")}`;
      return this.bytesText(this.progressTotalBytes);
    },
    progressTotalBytes() {
      if (this.finishHold && this.finishTotal) return this.finishTotal;
      if (this.chain && this.chain.totalBytes) return this.chain.totalBytes;
      return this.delivery ? this.delivery.totalBytes : 0;
    },
    rightMeta() {
      if (this.finishHold) return "100%";
      if (this.delivery && this.delivery.state === "downloading" && this.delivery.speedBps)
        return `${this.percent}% · ${this.speedText} · ${this.etaText}`;
      return this.percent + "%";
    },
    speedText() {
      let mb = (this.delivery.speedBps || 0) / (1024 * 1024);
      return this.$t("#L_D_SPEED", formatNumber(mb, 1, this.$t));
    },
    etaText() {
      let seconds = this.delivery.etaSeconds;
      if (!seconds) return "";

      if (seconds < 60) return this.$t("#L_D_ETA", `${seconds} ${this.$t("#L_D_SEC")}`);

      let minutes = Math.round(seconds / 60);
      return this.$t("#L_D_ETA", `${minutes} ${this.$t("#L_D_MIN")}`);
    },
    installed() {
      return !!(this.delivery && this.delivery.installedVersion);
    },

    button() {
      if (!this.delivery || !this.delivery.mainButton)
        return { state: "ready", action: "play", enabled: true };

      return episodeButton(this.delivery.mainButton, this.delivery, this.episodeId);
    },
    buttonState() {
      return this.button.state;
    },
    buttonEnabled() {
      return this.button.enabled !== false;
    },
    buttonView() {
      return BUTTON[this.buttonState] || BUTTON.ready;
    },
    actionText() {
      return this.$t(this.buttonView.key);
    },
    actionIcon() {
      return this.buttonView.icon;
    },
    buttonTone() {
      return TONES[this.buttonState] || "ready";
    },
  },
  watch: {
    deliveryGameId(id) {
      this.clearFinishTimer();
      this.watched = { gameId: id, state: this.deliveryState, operation: this.delivery ? this.delivery.operation : null };
    },
    deliveryState(next) {
      const previous = this.watched;
      const current = { gameId: this.deliveryGameId, state: next, operation: this.delivery ? this.delivery.operation : null };
      this.watched = current;

      if (progress.shouldHoldFinish(previous, current)) {
        this.clearFinishTimer();
        this.finishHold = true;
        this.finishState = next;
        // К концу операции main уже снял её с карточки — берём из прошлого состояния.
        this.finishOperation = (this.delivery && this.delivery.operation) || previous.operation || null;
        this.finishTotal = this.progressTotalBytes;
        this.finishTimer = setTimeout(() => {
          this.finishHold = false;
          this.finishState = null;
          this.finishOperation = null;
          this.finishTotal = 0;
          this.finishTimer = null;
        }, FINISH_HOLD_MS);
        return;
      }

      if (!progress.isSuccess(next)) this.clearFinishTimer();
    },
  },
  methods: {
    clearFinishTimer() {
      if (this.finishTimer) {
        clearTimeout(this.finishTimer);
        this.finishTimer = null;
      }
      this.finishHold = false;
    },
    onAction() {
      const action = this.button.action;

      if (!this.buttonEnabled || ACTIONS.indexOf(action) === -1) return;

      // «Скачать эпизод» и «Продолжить» эпизода — свой канал, а не установка Ghosting.
      if (this.buttonState === "episode-not-installed" || this.buttonState === "episode-paused")
        return this.$emit("install-episode", this.episodeId);

      this.$emit(action);
    },
    errorText(code) {
      let key = "#L_D_ERR_" + code;
      let text = this.$t(key);
      return text === key ? this.$t("#L_D_ERROR") : text;
    },
    sizeText(mbytes) {
      if (mbytes >= 1000)
        return (
          formatNumber(mbytes / 1000, 2, this.$t) + " " + this.$t("#L_GB")
        );
      return mbytes + " " + this.$t("#L_MB");
    },
    bytesText(bytes) {
      // Всего меньше мегабайта (докачка пары cfg) — в КБ, а не «0 МБ / 0 МБ».
      const total = this.progressTotalBytes;
      if (total > 0 && total < 1024 * 1024) return Math.ceil((bytes || 0) / 1024) + " " + this.$t("#L_KB");
      let mbytes = Math.round((bytes || 0) / (1024 * 1024));
      return this.sizeText(mbytes);
    },
  },
  beforeUnmount() {
    this.clearFinishTimer();
  },
};
</script>

<style lang="scss" scoped>
.download-bar__cancel {
  width: 44px;
  height: 44px;
  flex: none;
  align-self: center;
  border: 0;
  border-radius: var(--l-radius-sm);
  background: transparent;
  color: var(--l-text-dim);
  font-size: 18px;
  cursor: pointer;
  &:hover { color: #ff8f84; background: rgba(255, 255, 255, 0.06); }
  &:focus-visible { outline: 2px solid var(--accent-focus); }
}
.download-bar {
  position: relative;
  height: var(--l-bottombar-h);
  min-height: var(--l-bottombar-h);
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--l-bottombar-gap);
  padding: 0 var(--l-edge-pad) 0 var(--l-hero-pad);
  background: transparent;
  z-index: 6;

  &::before {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    top: calc(-1 * var(--l-footer-feather));
    background: var(--l-footer-fill);
    -webkit-mask-image: linear-gradient(90deg, transparent 0, #000 var(--l-footer-lead-in));
    mask-image: linear-gradient(90deg, transparent 0, #000 var(--l-footer-lead-in));
    pointer-events: none;
    z-index: -1;
  }

  &__status {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 20px;
    min-width: 0;
  }

  &__icon {
    width: 56px;
    height: 56px;
    min-width: 56px;
    border-radius: var(--l-radius-sm);
    box-shadow: inset 0 0 0 1px var(--l-border-strong);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--l-text-dim);
    font-size: 21px;

    i {
      width: 24px;
      line-height: 24px;
      text-align: center;
    }
  }

  /* Подпись 20 + 8 + полоса 8 + 8 + цифры 20 = 64: блок кратен 8 и
     центрируется в панели на сетке, полоса стоит на целом пикселе */
  &__info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  &__label {
    color: var(--l-text);
    font-size: 14px;
    font-weight: 500;
    line-height: 20px;
  }

  &__track {
    height: 8px;
    background: rgba(255, 255, 255, 0.09);
    border-radius: 4px;
    overflow: hidden;
  }

  &__fill {
    height: 100%;
    background: var(--accent);
    border-radius: 4px;
    transition: width var(--l-med);

    &.done {
      opacity: 0.35;
    }
  }

  &__meta {
    display: flex;
    justify-content: space-between;
    color: var(--l-text-dim);
    font-size: 12.5px;
    line-height: 20px;
  }

  &__controls {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--l-control-gap);
  }

  /* Один зазор между запуском, меню слева и выбором сборки сверху. */
  &__build {
    position: absolute;
    right: 0;
    bottom: calc(100% + var(--l-control-gap));
    z-index: 3;
  }

  /* Размеры — ступени сетки (launcher.css). Высота строки задана, чтобы
     надпись стояла в кнопке на целом пикселе. */
  /* Значок и надпись центрируются вместе. Ширину пары v-grid-width доводит
     до кратной 4, а значку задана целая ширина, иначе надпись начиналась
     с дробного x и значок мылился. */
  &__play-label {
    display: flex;
    align-items: center;
    gap: 16px;

    i {
      width: 24px;
      font-size: var(--l-play-icon);
      line-height: 24px;
      text-align: center;
    }
  }

  &__play {
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--l-play-w);
    height: var(--l-play-h);
    flex: none;
    padding: 0;
    border: none;
    border-radius: var(--l-radius);
    background: var(--tone-bg);
    color: var(--tone-text);
    font-family: inherit;
    font-size: var(--l-play-font);
    font-weight: 700;
    line-height: 24px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    cursor: pointer;
    box-shadow: 0 6px 22px var(--tone-glow);
    transition: background var(--l-fast), transform var(--l-fast),
      box-shadow var(--l-med);

    &:hover {
      background: var(--tone-hover);
      box-shadow: 0 8px 26px var(--tone-glow);
    }

    &:active {
      background: var(--tone-pressed);
      transform: scale(0.98);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: 4px;
    }

    &.tone-ready {
      --tone-bg: var(--btn-ready-bg);
      --tone-hover: var(--btn-ready-hover);
      --tone-pressed: var(--btn-ready-pressed);
      /* Светлые темы лаунчера (Steam 2003, Полночь) ставят тёмный текст. */
      --tone-text: var(--btn-ready-text, #ffffff);
      --tone-glow: var(--btn-ready-glow);
    }

    &.tone-progress {
      --tone-bg: var(--btn-progress-bg);
      --tone-hover: var(--btn-progress-hover);
      --tone-pressed: var(--btn-progress-pressed);
      --tone-text: var(--btn-progress-text, #ffffff);
      --tone-glow: var(--btn-progress-glow);
    }

    &.tone-repair {
      --tone-bg: var(--btn-repair-bg);
      --tone-hover: var(--btn-repair-hover);
      --tone-pressed: var(--btn-repair-pressed);
      --tone-text: #241a05;
      --tone-glow: var(--btn-repair-glow);
    }

    &.tone-danger {
      --tone-bg: var(--btn-danger-bg);
      --tone-hover: var(--btn-danger-hover);
      --tone-pressed: var(--btn-danger-pressed);
      --tone-text: #ffffff;
      --tone-glow: var(--btn-danger-glow);
    }

    &.tone-muted {
      --tone-bg: var(--btn-muted-bg);
      --tone-hover: var(--btn-muted-hover);
      --tone-pressed: var(--btn-muted-hover);
      --tone-text: var(--l-text-dim);
      --tone-glow: transparent;
      box-shadow: inset 0 0 0 1px var(--l-border-strong);
    }

    &.tone-neutral {
      --tone-bg: rgba(255, 255, 255, 0.08);
      --tone-hover: rgba(255, 255, 255, 0.13);
      --tone-pressed: rgba(255, 255, 255, 0.16);
      --tone-text: var(--l-text);
      --tone-glow: transparent;
      box-shadow: inset 0 0 0 1px var(--l-border-strong);
    }

    &.is-busy {
      cursor: default;
      opacity: 0.75;
      box-shadow: none;

      &:hover {
        background: var(--tone-bg);
        box-shadow: none;
      }

      &:active {
        transform: none;
      }
    }
  }
}

/* Блок прогресса появляется мгновенно, а уходит плавно — и после ухода
   удаляется из разметки (v-if), поэтому места не занимает */
.db-status-leave-active {
  transition: opacity var(--l-med), transform var(--l-med);
}

.db-status-leave-to {
  opacity: 0;
  transform: translateY(8px);
}

.db-status-enter-active {
  transition: opacity var(--l-fast);
}

.db-status-enter-from {
  opacity: 0;
}

.no-anim .db-status-leave-active,
.no-anim .db-status-enter-active {
  transition: none;
}
</style>
