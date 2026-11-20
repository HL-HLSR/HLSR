<template>
  <section class="look-code">
    <header class="look-code__head">
      <h3 class="look-code__title">{{ $t("#L_PZ_CODE_TITLE") }}</h3>
      <p class="look-code__lead">{{ $t("#L_PZ_CODE_LEAD", gameLabel) }}</p>
    </header>

    <div class="look-code__forms">
      <div class="look-code__form">
        <label class="look-code__label" :for="'look-code-export-' + gameId">{{ $t("#L_PZ_CODE_YOURS") }}</label>
        <p class="look-code__hint">{{ $t("#L_PZ_CODE_EXPORT_HINT") }}</p>
        <textarea
          :id="'look-code-export-' + gameId"
          class="look-code__field"
          :value="exportCode"
          :placeholder="$t('#L_PZ_CODE_EXPORT_PLACEHOLDER')"
          readonly
          spellcheck="false"
          @focus="$event.target.select()"
        ></textarea>
        <button type="button" class="look-code__button" :disabled="disabled || !!busy || invalid" @click="copyCode">
          <i :class="busy === 'export' ? 'fas fa-spinner fa-spin' : copied ? 'fas fa-check' : 'far fa-copy'" aria-hidden="true"></i>
          {{ $t(copied ? "#L_PZ_CODE_COPIED" : "#L_PZ_CODE_COPY") }}
        </button>
      </div>

      <form class="look-code__form" @submit.prevent="importCode">
        <label class="look-code__label" :for="'look-code-import-' + gameId">{{ $t("#L_PZ_CODE_IMPORT_LABEL") }}</label>
        <p class="look-code__hint">{{ $t("#L_PZ_CODE_IMPORT_HINT") }}</p>
        <textarea
          :id="'look-code-import-' + gameId"
          v-model="codeInput"
          class="look-code__field"
          :class="{ 'is-invalid': !!importError }"
          :placeholder="$t('#L_PZ_CODE_IMPORT_PLACEHOLDER')"
          :maxlength="maxLength"
          :disabled="disabled || !!busy"
          :aria-invalid="importError ? 'true' : null"
          :aria-describedby="importError ? 'look-code-error-' + gameId : null"
          spellcheck="false"
          autocapitalize="off"
          autocomplete="off"
          @input="importError = ''; modNotes = []"
        ></textarea>
        <button type="submit" class="look-code__button is-primary" :disabled="disabled || !!busy || !codeInput.trim()">
          <i :class="busy === 'import' ? 'fas fa-spinner fa-spin' : 'fas fa-file-import'" aria-hidden="true"></i>
          {{ $t("#L_PZ_CODE_IMPORT") }}
        </button>
        <p v-if="importError" :id="'look-code-error-' + gameId" class="look-code__error" role="alert">{{ importError }}</p>
        <p v-for="note in modNotes" :key="note.text" :class="note.error ? 'look-code__error' : 'look-code__note'" :role="note.error ? 'alert' : 'status'">{{ note.text }}</p>
      </form>
    </div>

    <p v-if="exportError" class="look-code__error" role="alert">{{ exportError }}</p>
    <p v-if="invalid" class="look-code__error">{{ $t("#L_GS_MOD_INVALID") }}</p>
    <p class="look-code__files">{{ $t("#L_PZ_CODE_MODS_NOTE", $t(gameId === "70" ? "#L_CPK_TAB_HL1" : "#L_CPK_TAB_GHOSTING")) }}</p>
    <p v-if="gameId === '218'" class="look-code__files">{{ $t("#L_PZ_CODE_FILES") }}</p>
  </section>
</template>

<script>
import { ERRORS as PACK_ERRORS } from "./CustomPacks";
const { ipcRenderer, clipboard } = window.hlsrElectron;
const { MAX_CODE_LENGTH } = require("../../../shared/personalization-code");

const ERROR_KEYS = {
  PERSONALIZATION_CODE_WRONG_GAME: "#L_PZ_CODE_WRONG_GAME",
  PERSONALIZATION_CODE_VERSION_UNSUPPORTED: "#L_PZ_CODE_VERSION",
  PERSONALIZATION_CODE_TOO_LARGE: "#L_PZ_CODE_TOO_LARGE",
};

export default {
  name: "GamePersonalizationCode",
  emits: ["import"],
  props: {
    gameId: { type: String, required: true },
    look: { type: Object, required: true },
    invalid: { type: Boolean, default: false },
    disabled: { type: Boolean, default: false },
  },
  data() {
    return { exportCode: "", codeInput: "", busy: "", copied: false, exportError: "", importError: "", modNotes: [], maxLength: MAX_CODE_LENGTH };
  },
  computed: {
    gameLabel() { return this.gameId === "70" ? "Half-Life" : "HL2: Ghosting"; },
  },
  watch: {
    look: { deep: true, handler() { this.exportRevision += 1; this.exportCode = ""; this.copied = false; this.exportError = ""; } },
  },
  created() { this.alive = true; this.exportRevision = 0; },
  beforeUnmount() { this.alive = false; },
  methods: {
    async copyCode() {
      if (this.busy || this.disabled || this.invalid) return;
      this.busy = "export";
      this.exportError = "";
      this.copied = false;
      const revision = this.exportRevision;
      try {
        // Реактивный черновик Vue нельзя передать через structured clone IPC.
        const look = JSON.parse(JSON.stringify(this.look));
        const result = await ipcRenderer.invoke("game-personalization-code-export", this.gameId, look);
        if (!this.alive || this.exportRevision !== revision) return;
        if (!result || result.status !== "ok") {
          this.exportError = this.$t("#L_PZ_CODE_EXPORT_FAILED");
          return;
        }
        this.exportCode = result.code;
        const copied = await clipboard.writeText(result.code);
        if (!this.alive || this.exportRevision !== revision) return;
        if (!copied || copied.status !== "ok") {
          this.exportError = this.$t("#L_PZ_CODE_COPY_FAILED");
          return;
        }
        this.copied = true;
        ipcRenderer.invoke("runner-secret-unlock", "frankenstein").catch(() => {});
      } catch (error) {
        if (this.alive) this.exportError = this.$t(this.exportCode ? "#L_PZ_CODE_COPY_FAILED" : "#L_PZ_CODE_EXPORT_FAILED");
      } finally {
        if (this.alive) this.busy = "";
      }
    },
    async importCode() {
      if (this.busy || this.disabled || !this.codeInput.trim()) return;
      this.busy = "import";
      this.importError = "";
      this.modNotes = [];
      const gameId = this.gameId;
      try {
        const result = await ipcRenderer.invoke("game-personalization-code-import", gameId, this.codeInput);
        if (!this.alive || this.gameId !== gameId) return;
        if (!result || result.status !== "ok") {
          this.importError = this.$t(ERROR_KEYS[result && result.error && result.error.code] || "#L_PZ_CODE_INVALID");
          return;
        }
        const packs = result.mods ? await this.applyMods(result.mods) : null;
        if (!this.alive || this.gameId !== gameId) return;
        this.$emit("import", { gameId, look: result.look, packs });
      } catch (error) {
        if (this.alive) this.importError = this.$t("#L_PZ_CODE_IMPORT_FAILED");
      } finally {
        if (this.alive) this.busy = "";
      }
    },
    // Моды из кода переключаются сразу, как на вкладке модов. Чего не вышло, то
    // названо под формой; вид из кода импортируется в любом случае. Возвращает список модов с диска.
    async applyMods(mods) {
      const title = (id) => this.$t("#L_CPK_TITLE_" + id.replace(/-/g, "_").toUpperCase());
      const current = await ipcRenderer.invoke("custom-packs-get", this.gameId).catch(() => null);
      let packs = current && current.status === "ok" ? current.packs : null;
      const switched = [], missing = [];
      let failed = !packs ? "CUSTOM_PACK_FAILED" : current.installed === false ? "NOT_INSTALLED" : "";
      for (const pack of failed ? [] : packs) {
        const want = mods[pack.id];
        if (typeof want !== "boolean" || want === pack.enabled) continue;
        if (want && !pack.available) { missing.push(title(pack.id)); continue; }
        const result = await ipcRenderer.invoke("custom-packs-set", this.gameId, pack.id, want).catch(() => null);
        if (result && result.packs) packs = result.packs;
        if (!result || result.status !== "ok") { failed = (result && result.code) || "CUSTOM_PACK_FAILED"; break; }
        switched.push(this.$t(want ? "#L_PZ_CODE_MOD_ON" : "#L_PZ_CODE_MOD_OFF", title(pack.id)));
      }
      const notes = [];
      if (switched.length) notes.push({ text: this.$t("#L_PZ_CODE_MODS_DONE", switched.join(", ")) });
      if (missing.length) notes.push({ text: this.$t("#L_PZ_CODE_MODS_MISSING", missing.join(", ")), error: true });
      if (failed) notes.push({ text: this.$t("#L_PZ_CODE_MODS_FAILED", this.$t(PACK_ERRORS[failed] || "#L_CPK_ERR_FAILED")), error: true });
      if (this.alive) this.modNotes = notes;
      return packs;
    },
  },
};
</script>

<style lang="scss" scoped>
.look-code {
  padding: 20px;
  min-height: 100%;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius);
  color: var(--l-text);

  &__title { margin: 0 0 8px; font-size: 20px; font-weight: 600; line-height: 28px; }
  &__lead { max-width: 680px; margin: 0; color: var(--l-text-dim); font-size: 13px; line-height: 20px; }
  &__forms { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr)); gap: 20px; margin-top: 20px; }
  &__form { min-width: 0; display: flex; flex-direction: column; align-items: flex-start; }
  &__label { font-size: 14px; font-weight: 600; line-height: 20px; }
  &__hint { min-height: 36px; margin: 6px 0 12px; color: var(--l-text-dim); font-size: 12px; line-height: 18px; }
  &__field {
    display: block;
    width: 100%;
    height: 128px;
    min-height: 100px;
    padding: 12px;
    resize: vertical;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-bar);
    color: var(--l-text);
    font: 12px/1.6 Consolas, "Liberation Mono", monospace;
    overflow-wrap: anywhere;
    word-break: break-all;
    transition: border-color var(--l-fast);
    &::placeholder { color: var(--l-text-dim); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
    &.is-invalid { border-color: var(--l-error, #ff8a98); }
  }
  &__button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 36px;
    padding: 0 16px;
    margin-top: 12px;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-bar);
    color: var(--l-text);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast), border-color var(--l-fast), color var(--l-fast);
    &:hover:not(:disabled) { border-color: var(--l-border-strong); background: var(--l-surface); }
    &.is-primary { border-color: var(--accent-border); background: var(--accent-soft); }
    &.is-primary:hover:not(:disabled) { border-color: var(--accent); background: var(--accent-soft-hover, var(--accent-soft)); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
    &:disabled { opacity: 0.45; cursor: default; }
  }
  &__error { margin: 12px 0 0; color: var(--l-error, #ff8a98); font-size: 12px; line-height: 18px; }
  &__note { margin: 12px 0 0; color: var(--l-text-dim); font-size: 12px; line-height: 18px; }
  &__files { margin: 20px 0 0; color: var(--l-text-dim); font-size: 12px; line-height: 18px; }
}
</style>
