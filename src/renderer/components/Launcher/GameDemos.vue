<template>
  <div class="gd">
    <div class="gd__head">
      <span class="gd__hint">{{ summary }}</span>
      <span class="gd__spacer"></span>
      <button type="button" class="gd__btn" :disabled="loading || packing" @click="load">
        <i class="fas fa-sync-alt" aria-hidden="true"></i>{{ $t("#L_GS_REFRESH") }}
      </button>
      <button v-if="canDelete" type="button" class="gd__btn" @click="openFolder">
        <i class="fas fa-folder-open" aria-hidden="true"></i>{{ $t("#L_GS_OPEN_DEMOS") }}
      </button>
      <button type="button" class="gd__btn" :disabled="!demos.length || packing" @click="setAll(true)">
        {{ $t("#L_DEMOS_SELECT_ALL") }}
      </button>
      <button type="button" class="gd__btn" :disabled="!chosenCount || packing" @click="setAll(false)">
        {{ $t("#L_DEMOS_CLEAR_ALL") }}
      </button>
    </div>

    <div v-if="loading && !demos.length" class="gd__empty">{{ $t("#L_GS_LOADING") }}</div>
    <div v-else-if="listError" class="gd__empty" role="alert">{{ listError }}</div>
    <div v-else-if="!demos.length" class="gd__empty">{{ $t("#L_DEMOS_EMPTY") }}</div>
    <ul v-else class="gd__list">
      <li v-for="demo in demos" :key="keyOf(demo)" class="gd__row">
        <label class="gd__pick">
          <input type="checkbox" :checked="!!chosen[keyOf(demo)]" :disabled="packing" @change="toggle(demo)" />
          <i class="fas fa-film" aria-hidden="true"></i>
          <span class="gd__name">{{ demo.name }}</span>
        </label>
        <span class="gd__meta">{{ formatDate(demo.mtimeMs) }}</span>
        <span class="gd__meta">{{ formatSize(demo.size) }}</span>
        <button v-if="canCopy" type="button" class="gd__icon"
          :aria-label="$t('#L_GS_DEMO_COPY', command(demo))" :title="$t('#L_GS_DEMO_COPY', command(demo))"
          @click="copy(demo)">
          <i :class="copied === keyOf(demo) ? 'fas fa-check' : 'fas fa-terminal'" aria-hidden="true"></i>
        </button>
        <span v-if="canDelete" class="gd__actions">
          <template v-if="pendingDelete === keyOf(demo)">
            <span class="gd__confirm">{{ $t("#L_GS_GHOST_DELETE_CONFIRM") }}</span>
            <button type="button" class="gd__icon danger" :disabled="!!deleting"
              :aria-label="$t('#L_GS_GHOST_DELETE_YES')" :title="$t('#L_GS_GHOST_DELETE_YES')" @click="remove(demo)">
              <i :class="deleting === keyOf(demo) ? 'fas fa-spinner fa-spin' : 'fas fa-check'" aria-hidden="true"></i>
            </button>
            <button type="button" class="gd__icon" :disabled="!!deleting"
              :aria-label="$t('#L_CANCEL')" :title="$t('#L_CANCEL')" @click="pendingDelete = null">
              <i class="fas fa-times" aria-hidden="true"></i>
            </button>
          </template>
          <button v-else-if="isUnique(demo)" type="button" class="gd__icon" :disabled="!!deleting"
            :aria-label="$t('#L_GS_DEMO_DELETE') + ': ' + demo.name" :title="$t('#L_GS_DEMO_DELETE')"
            @click="pendingDelete = keyOf(demo)">
            <i class="fas fa-trash-alt" aria-hidden="true"></i>
          </button>
        </span>
      </li>
    </ul>
    <p v-if="deleteError" class="gd__error" role="alert">{{ deleteError }}</p>

    <div class="gd__pack">
      <button type="button" class="gd__btn primary" :disabled="!chosenCount || packing" @click="pack">
        <i :class="packing ? 'fas fa-spinner fa-spin' : 'fas fa-file-archive'" aria-hidden="true"></i>
        {{ packing ? $t("#L_DEMOS_PACKING") + (packPercent > 0 ? " " + packPercent + "%" : "") : $t("#L_DEMOS_PACK", String(chosenCount)) }}
      </button>
      <p v-if="packError" class="gd__error" role="alert">{{ packError }}</p>
      <div v-if="archive" class="gd__archive">
        <span class="gd__archive-name">{{ $t("#L_DEMOS_ARCHIVE", archive.name, formatSize(archive.size)) }}</span>
        <button type="button" class="gd__btn" @click="reveal">
          <i class="fas fa-folder-open" aria-hidden="true"></i>{{ $t("#L_DEMOS_REVEAL") }}
        </button>
        <button type="button" class="gd__btn" @click="openLink(speedrunKey)">
          <i class="fas fa-external-link-alt" aria-hidden="true"></i>speedrun.com
        </button>
        <button type="button" class="gd__btn" @click="openLink('google-drive')">
          <i class="fab fa-google-drive" aria-hidden="true"></i>Google Drive
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import { formatSize, formatDate } from "./file-format";

const { ipcRenderer } = window.hlsrElectron;
const DAY = 24 * 3600 * 1000;
const PACK_ERRORS = {
  DEMO_NOT_FOUND: "#L_DEMOS_ERR_CHANGED",
  PACK_BUSY: "#L_DEMOS_ERR_BUSY",
  NOT_INSTALLED: "#L_GS_NOT_INSTALLED",
};

// Раздел «Демо»: выбор записей и упаковка в zip. Черновик настроек не трогает.
export default {
  name: "GameDemos",
  props: {
    gameId: { type: String, required: true },
    // "goldsrc" или "ghosting": у HL1 есть удаление и playdemo, у Ghosting нет.
    engine: { type: String, required: true },
    active: { type: Boolean, default: false },
  },
  data() {
    return {
      demos: [],
      chosen: {},
      loading: false,
      listError: null,
      packing: false,
      packPercent: 0,
      packError: null,
      archive: null,
      copied: null,
      pendingDelete: null,
      deleting: null,
      deleteError: null,
    };
  },
  computed: {
    canDelete() { return this.engine === "goldsrc"; },
    canCopy() { return this.engine === "goldsrc"; },
    speedrunKey() { return this.engine === "goldsrc" ? "speedrun-hl1" : "speedrun-hl2"; },
    chosenCount() { return this.demos.filter((demo) => this.chosen[this.keyOf(demo)]).length; },
    summary() {
      return this.loading ? this.$t("#L_GS_LOADING") : this.$t("#L_GS_DEMOS_COUNT", String(this.demos.length));
    },
  },
  watch: {
    active(value) { if (value) this.load(); },
    gameId() { this.archive = null; this.packError = null; if (this.active) this.load(); },
  },
  mounted() { if (this.active) this.load(); },
  beforeUnmount() { clearTimeout(this.copiedTimer); ipcRenderer.removeListener("game-demos-pack-progress", this.onPackProgress); },
  methods: {
    keyOf(demo) { return `${demo.dir}|${demo.name}`; },
    // Удаление идёт по имени (на Windows без учёта регистра): при одинаковых именах в двух папках кнопки нет.
    isUnique(demo) {
      const name = demo.name.toLowerCase();
      return this.demos.filter((item) => item.name.toLowerCase() === name).length === 1;
    },
    onPackProgress(event, payload) {
      if (payload && payload.gameId === this.gameId) this.packPercent = payload.percent;
    },
    command(demo) { return `playdemo ${demo.name.replace(/\.dem$/i, "")}`; },
    copy(demo) {
      window.hlsrElectron.clipboard.writeText(this.command(demo));
      this.copied = this.keyOf(demo);
      clearTimeout(this.copiedTimer);
      this.copiedTimer = setTimeout(() => { this.copied = null; }, 1500);
    },
    formatSize(bytes) { return formatSize(bytes, this.$t); },
    formatDate,
    toggle(demo) {
      const key = this.keyOf(demo);
      this.chosen = { ...this.chosen, [key]: !this.chosen[key] };
    },
    setAll(value) {
      const next = {};
      for (const demo of this.demos) next[this.keyOf(demo)] = value;
      this.chosen = next;
    },
    async load() {
      if (this.loading) return;
      this.loading = true;
      this.listError = null;
      this.pendingDelete = null;
      try {
        const result = await ipcRenderer.invoke("game-demos-list", this.gameId);
        if (result && result.status === "ok") {
          const known = this.chosen;
          const next = {};
          const since = Date.now() - DAY;
          // Выбор пользователя переживает обновление; новые демо отмечаются, если им нет суток.
          for (const demo of result.demos || []) {
            const key = this.keyOf(demo);
            next[key] = key in known ? known[key] : demo.mtimeMs >= since;
          }
          this.demos = result.demos || [];
          this.chosen = next;
        } else {
          this.demos = [];
          this.chosen = {};
          this.listError = result && result.code === "NOT_INSTALLED"
            ? this.$t("#L_GS_NOT_INSTALLED") : this.$t("#L_GS_RUNS_ERROR", (result && result.code) || "");
        }
      } catch (error) {
        this.demos = [];
        this.listError = this.$t("#L_GS_RUNS_ERROR", "");
      } finally {
        this.loading = false;
      }
    },
    async pack() {
      if (this.packing || !this.chosenCount) return;
      this.packing = true;
      this.packPercent = 0;
      ipcRenderer.on("game-demos-pack-progress", this.onPackProgress);
      this.packError = null;
      this.archive = null;
      const items = this.demos
        .filter((demo) => this.chosen[this.keyOf(demo)])
        .map((demo) => ({ dir: demo.dir, name: demo.name }));
      try {
        const result = await ipcRenderer.invoke("game-demos-pack", this.gameId, items);
        if (result && result.status === "ok" && result.archive) {
          this.archive = result.archive;
          this.reveal();
        } else {
          const code = result && result.code;
          this.packError = this.$t(PACK_ERRORS[code] || "#L_DEMOS_ERR_PACK");
          if (code === "DEMO_NOT_FOUND") {
            this.packing = false;
            await this.load();
          }
        }
      } catch (error) {
        this.packError = this.$t("#L_DEMOS_ERR_PACK");
      } finally {
        ipcRenderer.removeListener("game-demos-pack-progress", this.onPackProgress);
        this.packing = false;
      }
    },
    openFolder() {
      ipcRenderer.invoke("game-runs-open-folder", this.gameId);
    },
    reveal() {
      if (this.archive) ipcRenderer.invoke("game-demos-archive-reveal", this.archive.name);
    },
    openLink(key) {
      ipcRenderer.invoke("open-external-link", key);
    },
    async remove(demo) {
      const key = this.keyOf(demo);
      if (this.deleting || this.pendingDelete !== key) return;
      this.deleting = key;
      this.deleteError = null;
      try {
        const result = await ipcRenderer.invoke("game-runs-delete", this.gameId, demo.name);
        if (!result || result.status !== "ok") {
          this.deleteError = result && result.code === "GAME_RUNNING"
            ? this.$t("#L_D_ERR_GAME_RUNNING") : this.$t("#L_GS_DEMO_DELETE_FAILED");
          return;
        }
        this.demos = this.demos.filter((item) => this.keyOf(item) !== key);
        this.pendingDelete = null;
      } catch (error) {
        this.deleteError = this.$t("#L_GS_DEMO_DELETE_FAILED");
      } finally {
        this.deleting = null;
      }
    },
  },
};
</script>

<style lang="scss" scoped>
.gd {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;

  &__head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  &__spacer { flex: 1; }
  &__hint { color: var(--l-text-dim); font-size: 12.5px; }

  &__empty {
    padding: 32px 16px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.02);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-faint);
    font-size: 13px;
    text-align: center;
  }

  &__list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }

  &__row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 44px;
    padding: 4px 8px 4px 12px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    color: var(--l-text);
    font-size: 13px;
  }

  &__pick {
    display: flex; align-items: center; gap: 10px; flex: 1; min-width: 0; cursor: pointer;
    input { accent-color: var(--l-accent); margin: 0; }
    i { color: var(--l-text-dim); }
  }
  &__name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  &__meta { flex: none; color: var(--l-text-faint); font-size: 12px; font-variant-numeric: tabular-nums; }
  &__actions { display: flex; align-items: center; gap: 4px; flex: none; }
  &__confirm { color: var(--l-text-dim); font-size: 12px; }

  &__icon {
    display: grid; place-items: center; width: 32px; height: 32px; flex: none; padding: 0;
    border: 0; border-radius: var(--l-radius-sm); background: transparent; color: var(--l-text-dim); cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);
    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
    &.danger i { color: #ff8796; }
    &:disabled { opacity: 0.5; cursor: default; }
  }

  &__btn {
    display: inline-flex; align-items: center; justify-content: center; gap: 8px;
    min-height: 36px; padding: 6px 14px;
    border: 1px solid var(--l-border-strong); border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05); color: var(--l-text);
    font-family: inherit; font-size: 13px; font-weight: 600; cursor: pointer;
    transition: background var(--l-fast);
    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
    &:disabled { opacity: 0.5; cursor: default; }
    &.primary { background: var(--l-accent-soft); color: var(--l-accent); }
  }
  &__btn:focus-visible, &__icon:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }

  &__pack { display: flex; flex-direction: column; align-items: flex-start; gap: 10px; }
  &__archive { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; color: var(--l-text-dim); font-size: 12.5px; }
  &__archive-name { min-width: 0; overflow-wrap: anywhere; }
  &__error { margin: 0; color: #ff8796; font-size: 12px; }
}
</style>
