<template>
  <!-- Редактор внешнего вида одной игры (Ghosting или Half-Life). Черновики
       хранятся по игре (окно пересоздаётся при смене игры, страница спрашивает о несохранённом), сохраняются так же, как
       в окне настроек игры (game-settings-save). -->
  <div class="pz-editor">
    <div v-if="!drafts[gameId]" class="pz-editor__state">
      <i class="fas fa-spinner fa-spin" aria-hidden="true"></i>
      {{ $t("#L_GS_LOADING") }}
    </div>
    <template v-else>
      <div class="pz-editor__work">
        <!-- Подразделы редактора — пунктами списка страницы (section и nav), своего списка нет. -->
        <GhostingSettingsExtras
          :key="gameId"
          ref="extras"
          class="pz-editor__extras"
          :section="section"
          :value="editorValue"
          :game-id="gameId"
          :schema="isGoldSrc ? goldsrcSchema : null"
          :active="visible && active"
          :overlay="draft.look.liveSplitOverlay"
          @input="onEditorInput"
          @overlay="onOverlayInput"
          @validity="invalid[gameId] = !$event"
          @nav="$emit('nav', $event)"
        >
          <template #settings-code>
            <GamePersonalizationCode
              :game-id="gameId"
              :look="draft.look"
              :invalid="!!invalid[gameId]"
              :disabled="saving"
              @import="importCodeLook"
            />
          </template>
        </GhostingSettingsExtras>
      </div>
      <footer class="pz-editor__footer">
        <button type="button" class="pz-btn ghost" @click="resetGame">
          <i class="fas fa-undo" aria-hidden="true"></i>
          {{ $t("#L_PZ_RESET_GAME") }}
        </button>
        <span class="pz-editor__spacer"></span>
        <transition name="l-fade" mode="out-in">
          <span v-if="invalid[gameId]" key="invalid" class="pz-editor__note is-error" role="status">{{ $t("#L_GS_MOD_INVALID") }}</span>
          <span v-else-if="conflictShown" key="file-changed" class="pz-editor__note is-error" role="alert">{{ $t("#L_GS_CFG_CHANGED") }}</span>
          <span v-else-if="saveError" key="error" class="pz-editor__note is-error" role="alert">{{ saveError }}</span>
          <span v-else-if="justSaved === gameId" key="saved" class="pz-editor__note is-ok" role="status">
            <i class="fas fa-check" aria-hidden="true"></i> {{ $t("#L_PZ_SAVED") }}
          </span>
          <span v-else-if="codeImported === gameId" key="imported" class="pz-editor__note is-ok" role="status">{{ $t(isDirty(gameId) ? "#L_PZ_CODE_IMPORTED" : "#L_PZ_CODE_UNCHANGED") }}</span>
          <span v-else-if="isDirty(gameId)" key="dirty" class="pz-editor__note">{{ $t("#L_GS_X_APPLY_NOTE") }}</span>
        </transition>
        <template v-if="conflictShown">
          <button type="button" class="pz-btn" :disabled="saving" @click="reloadAfterChange">{{ $t("#L_GS_CFG_CHANGED_RELOAD") }}</button>
          <button type="button" class="pz-btn ghost" @click="conflictGameId = ''">{{ $t("#L_CANCEL") }}</button>
        </template>
        <button type="button" class="pz-btn" :disabled="!isDirty(gameId) || saving" @click="revertGame">
          {{ $t("#L_PZ_REVERT") }}
        </button>
        <button
          type="button"
          class="pz-btn primary"
          :disabled="!isDirty(gameId) || saving || invalid[gameId]"
          @click="save"
        >
          <i v-if="saving" class="fas fa-spinner fa-spin" aria-hidden="true"></i>
          {{ $t("#L_SAVE") }}
        </button>
      </footer>
    </template>
  </div>
</template>

<script>
import GhostingSettingsExtras from "./GhostingSettingsExtras";
import GamePersonalizationCode from "./GamePersonalizationCode";
import { defaultPersonalization, normalizePersonalization } from "./ghosting-personalization-defaults";
import { GOLDSRC_GAME_IDS } from "./launcher-data";
import { prepareLayers as prepareGhostingPreview } from "./ghosting-overlay-renderer";
import { loadLayers as prepareHl1Preview } from "./hl1-overlay-renderer";
import { resetExtras } from "../../../shared/ghosting-settings";
import { sanitizeOverlay, defaultOverlay } from "../../../shared/livesplit-overlay";
const goldsrcSchema = require("../../../shared/goldsrc-settings");

const { ipcRenderer } = window.hlsrElectron;

// Из сохранённых настроек игры редактору нужна только эта часть; остальное
// (запуск, бинды, язык) уходит при сохранении как было.
function pickLook(settings) {
  const source = settings || {};
  return {
    personalization: normalizePersonalization(source.personalization),
    packs: source.packs || null,
    bxt: source.bxt || null,
    // Оверлей LiveSplit в окне игры: тоже вид игры, хоть и не входит в код
    // персонализации (у каждого своё расположение LiveSplit).
    liveSplitOverlay: sanitizeOverlay(source.liveSplitOverlay),
  };
}

const snapshot = (value) => JSON.stringify(value);

export default {
  name: "GamePersonalization",
  components: { GhostingSettingsExtras, GamePersonalizationCode },
  emits: ["dirty", "nav"],
  props: {
    gameId: { type: String, required: true },
    // Раздел не на экране: предпросмотр и снимки LiveSplit не обновляются.
    active: { type: Boolean, default: true },
    // Подраздел редактора, выбранный в списке страницы.
    section: { type: String, default: "" },
  },
  data() {
    return {
      goldsrcSchema,
      // gameId → { settings, look, saved }: settings — всё, что вернул main,
      // look — черновик внешнего вида, saved — его снимок на момент загрузки.
      drafts: {},
      invalid: {},
      saving: false,
      saveError: "",
      conflictGameId: "",
      justSaved: "",
      codeImported: "",
      visible: true,
    };
  },
  computed: {
    // Конфликт файла относится к одной игре и виден только на её странице.
    conflictShown() {
      return !!this.conflictGameId && this.conflictGameId === this.gameId && !this.invalid[this.gameId];
    },
    isGoldSrc() {
      return GOLDSRC_GAME_IDS.includes(this.gameId);
    },
    draft() {
      return this.drafts[this.gameId] || null;
    },
    // Редактор HL1 видит паки и BXT одним плоским списком extra (как было в
    // окне настроек игры); у Ghosting extra — его собственные cvar мода.
    editorValue() {
      const look = this.draft.look;
      if (!this.isGoldSrc) return look.personalization;
      return Object.assign({}, look.personalization, { extra: goldsrcSchema.toFlat(look.packs, look.bxt) });
    },
    dirtyNow() {
      return this.isDirty(this.gameId);
    },
  },
  watch: {
    gameId: {
      immediate: true,
      handler() {
        this.saveError = "";
        if (!this.drafts[this.gameId]) this.load(this.gameId);
      },
    },
    dirtyNow: {
      immediate: true,
      handler(value) { this.$emit("dirty", value); },
    },
  },
  mounted() {
    // Предпросмотры готовятся заранее: иначе картинки, маски HUD и WebGL
    // (около 0,4 с) считаются прямо во время анимации перехода.
    this.prepareTimer = setTimeout(() => {
      prepareGhostingPreview().catch(() => {});
      prepareHl1Preview().catch(() => {});
    }, 800);
    window.addEventListener("focus", this.onWindowFocus);
  },
  // Вернулись на страницу (keep-alive): файл игрока могли сохранить на странице —
  // перечитываем его, как при возврате фокуса (R18).
  activated() {
    this.visible = true;
    this.onWindowFocus();
  },
  deactivated() { this.visible = false; },
  methods: {
    // После «Сбросить» есть что сохранить, даже если окно и так показывало умолчания.
    isDirty(id) {
      const entry = this.drafts[id];
      return !!entry && (snapshot(entry.look) !== entry.saved || !!entry.reset);
    },
    // Вернулись в лаунчер: файл игрока могли править руками. Активная игра без несохранённого черновика читается заново.
    // Ответ применяется, только если за время чтения черновик не правили, сохранение не шло и запрос последний.
    onWindowFocus() {
      const id = this.gameId;
      const idle = () => !this.saving && !this.conflictGameId && !!this.drafts[id] && !this.isDirty(id);
      if (this.visible === false || !idle()) return;
      this.focusToken = (this.focusToken || 0) + 1;
      const token = this.focusToken;
      return this.load(id, () => token === this.focusToken && idle());
    },
    async load(id, accept) {
      const result = await ipcRenderer.invoke("game-settings-get", id).catch(() => null);
      if (accept && !accept()) return;
      const settings = result && result.status === "ok" && result.settings ? result.settings : {};
      const look = pickLook(settings);
      this.drafts[id] = { settings, look, saved: snapshot(look), configSha256: result && result.configSha256 };
    },
    onEditorInput(value) {
      this.codeImported = "";
      const look = this.draft.look;
      if (!this.isGoldSrc) {
        look.personalization = value;
        return;
      }
      const split = goldsrcSchema.fromFlat(value.extra);
      look.personalization = Object.assign({}, value, { extra: look.personalization.extra });
      look.packs = split.packs;
      look.bxt = split.bxt;
    },
    onOverlayInput(value) {
      this.codeImported = "";
      this.draft.look.liveSplitOverlay = value;
    },
    // Сброс внешнего вида игры к стандартному. Сохранение после него пишет значения
    // по умолчанию в файл игрока явными строками (§5, reset): иначе в игре остались бы
    // значения, которые она сохранила в config.cfg.
    resetGame() {
      this.draft.reset = true;
      this.codeImported = "";
      const look = this.draft.look;
      const extra = resetExtras(look.personalization && look.personalization.extra);
      look.personalization = Object.assign(defaultPersonalization(), { extra });
      if (this.isGoldSrc) {
        look.packs = null;
        look.bxt = null;
      }
      // Выключатель оверлея — не внешний вид: сброс его не трогает.
      look.liveSplitOverlay = Object.assign(defaultOverlay(), { enabled: !!(look.liveSplitOverlay && look.liveSplitOverlay.enabled) });
    },
    // Черновик отброшен: сообщение о конфликте файла и ошибка сохранения к нему больше не относятся.
    revertGame() {
      this.codeImported = "";
      this.conflictGameId = "";
      this.saveError = "";
      this.draft.reset = false;
      this.draft.look = JSON.parse(this.draft.saved);
    },
    importCodeLook({ gameId, look, packs }) {
      if (gameId !== this.gameId || !this.draft || this.saving) return;
      // Моды из кода уже переключены: предпросмотр HUD узнаёт об этом сразу.
      if (packs && this.$refs.extras) this.$refs.extras.modPacks = packs;
      this.draft.look = Object.assign({}, this.draft.look, look);
      this.saveError = "";
      this.justSaved = "";
      this.codeImported = this.gameId;
      this.$refs.extras.selectSection("personalization");
    },
    // Файл игрока изменён вне лаунчера: черновик заменяется файлом только по выбору игрока.
    async reloadAfterChange() {
      const id = this.conflictGameId;
      if (!id) return;
      this.conflictGameId = "";
      this.saveError = "";
      if (this.codeImported === id) this.codeImported = "";
      await this.load(id);
    },
    async save() {
      const id = this.gameId;
      const entry = this.drafts[id];
      if (!entry || this.saving || this.invalid[id]) return;
      this.saving = true;
      this.focusToken = (this.focusToken || 0) + 1; // устаревший ответ перечитывания по фокусу отбрасывается
      this.saveError = "";
      this.conflictGameId = "";

      try {
        // Черновик — реактивный прокси Vue, IPC его не клонирует.
        const payload = JSON.parse(JSON.stringify(entry.look));
        const options = {};
        if (entry.configSha256 !== undefined) options.configSha256 = entry.configSha256;
        if (entry.reset) options.reset = true;
        const result = await ipcRenderer.invoke("game-settings-save", id, payload, "personalization",
          Object.keys(options).length ? options : undefined);
        if (!result || result.status !== "ok") {
          // Файл игрока правили вне лаунчера: настройки не сохранены, черновик остаётся до выбора «Перечитать» или «Отмена».
          if (result && result.error && result.error.code === "CONFIG_CHANGED") {
            this.conflictGameId = id;
            return;
          }
          this.saveError = this.$t("#L_PZ_SAVE_FAILED");
          return;
        }
        const settings = result.settings || payload;
        const look = pickLook(settings);
        this.drafts[id] = { settings, look, saved: snapshot(look), configSha256: result.configSha256 };
        this.justSaved = id;
        this.codeImported = "";
        clearTimeout(this.savedTimer);
        this.savedTimer = setTimeout(() => { this.justSaved = ""; }, 2400);
      } catch (error) {
        this.saveError = this.$t("#L_PZ_SAVE_FAILED");
      } finally {
        this.saving = false;
      }
    },
  },
  beforeUnmount() {
    clearTimeout(this.savedTimer);
    clearTimeout(this.prepareTimer);
    window.removeEventListener("focus", this.onWindowFocus);
  },
};
</script>

<style lang="scss" scoped>
/* Раздел страницы «Персонализация»: без своей карточки, как остальные разделы. */
.pz-editor {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;

  &__state {
    flex: 1;
    display: grid;
    place-content: center;
    grid-auto-flow: column;
    gap: 10px;
    color: var(--l-text-dim);
    font-size: 14px;
  }

  &__work {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* В окне настроек колонка полей была узкой (до 370 px), а предпросмотр
     делил с ней 1240 px. Здесь места больше: полям — до 440 px, остальное
     забирает предпросмотр. */
  &__extras {
    --gx-fields-w: clamp(340px, 30%, 440px);
    flex: 1;
    min-height: 0;
    height: auto;
  }

  /* Колонка настроек без подложки, но с тонкой рамкой, как у предпросмотра
     (просьба владельца 07.11: «границы видимые небольшие»). */
  :deep(.pers),
  :deep(.gx-pane),
  :deep(.look-code) {
    border: 1px solid var(--l-border);
    background: none;
  }

  /* Как нижняя строка остальных разделов: линия сверху, без подложки.
     В узкой колонке (маленькое окно и выдвинутая колонка значков) кнопки переносятся, а не обрезаются. */
  &__footer {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 16px 0 0;
    margin-top: 16px;
    box-shadow: inset 0 1px 0 var(--l-modal-border);
  }

  &__spacer { flex: 1; }

  &__note {
    color: var(--l-text-dim);
    font-size: 13px;

    &.is-error { color: #ff8a98; }
    &.is-ok { color: #7fe0a0; }
  }
}

/* Размеры как у кнопок нижней строки остальных разделов (.settings-btn): строка не прыгает
   при переходе между разделами. */
.pz-btn {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 48px;
  padding: 11px 19px;
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font: inherit;
  font-size: 13.5px;
  font-weight: 600;
  line-height: 24px;
  cursor: pointer;
  transition: background var(--l-fast), opacity var(--l-fast), transform var(--l-fast);

  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
  &:active:not(:disabled) { transform: scale(0.97); }
  &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
  &:disabled { opacity: 0.45; cursor: default; }

  &.ghost { background: transparent; color: var(--l-text-dim); }
  &.ghost:hover:not(:disabled) { color: var(--l-text); background: rgba(255, 255, 255, 0.06); }

  &.primary {
    border-color: transparent;
    background: var(--accent);
    color: var(--accent-contrast-text);

    &:hover:not(:disabled) { background: var(--accent-hover); }
  }
}
</style>
