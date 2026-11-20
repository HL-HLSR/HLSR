<template>
  <!-- «Персонализация»: настройки игры отдельной вкладкой шапки. Сверху выбор игры,
       ниже то же окно, что раньше открывала шестерёнка (GameSettingsModal в режиме
       embedded): разделы списком слева, импорт и экспорт в его нижней строке. -->
  <section class="launcher-main cfgp">
    <header class="cfgp__head" :inert="unsaved ? true : null">
      <h1 class="cfgp__title">{{ $t("#L_TAB_CONFIGS") }}</h1>
      <button
        v-if="games.length"
        type="button"
        class="cfgp__help"
        data-tour="configs-help"
        :title="$t('#L_CFG_HELP')"
        :aria-label="$t('#L_CFG_HELP')"
        @click="$emit('help', gameId)"
      ><i class="fas fa-question" aria-hidden="true"></i></button>

      <div v-if="games.length" class="cfgp__games" data-tour="configs-games" role="group" :aria-label="$t('#L_TAB_CONFIGS')">
        <!-- Half-Life 2 один: установлен — выбирается как все, нет — неактивный чип. LiveSplit — всегда неактивный. -->
        <button
          v-for="chip in chips"
          :key="chip.id || chip.name"
          type="button"
          class="cfgp__game"
          :class="{ active: !chip.disabled && chip.id === gameId, 'is-soon': chip.disabled }"
          :aria-pressed="chip.disabled ? null : chip.id === gameId ? 'true' : 'false'"
          :aria-disabled="chip.disabled ? 'true' : null"
          :title="chip.disabled ? $t('#L_BUILD_SOON') : null"
          :disabled="!chip.disabled && busy"
          @click="chip.disabled || requestGame(chip.id)"
        >{{ chip.name }}<span v-if="chip.soon" class="cfgp__soon">{{ $t("#L_CFG_HL2_SOON") }}</span></button>
      </div>

    </header>

    <p
      v-if="notice"
      class="cfgp__notice"
      :class="{ 'is-error': notice.error }"
      role="status"
      :title="notice.details || null"
    >
      <span>{{ notice.text }}</span>
      <button v-if="notice.rules" type="button" class="settings-btn cfgp__compact" @click="showRules">
        {{ $t("#L_HC_SHOW") }}
      </button>
      <button v-if="notice.undo" type="button" class="settings-btn cfgp__compact" :disabled="busy" @click="undoImport">
        {{ $t("#L_CFG_IMPORT_UNDO") }}
      </button>
    </p>

    <div v-if="!games.length" class="cfgp__empty">
      <p>{{ $t("#L_CFG_NO_GAMES") }}</p>
      <button type="button" class="settings-btn primary" @click="$emit('open-games')">{{ $t("#L_TAB_GAMES") }}</button>
    </div>

    <div v-else class="cfgp__body" :inert="unsaved ? true : null">
      <GameSettingsModal
        :key="gameId"
        ref="settings"
        embedded
        :gameId="gameId"
        :gameTitle="gameTitle"
        :initial-section="openSection"
        :ask-unsaved="settleUnsaved"
        @saved="onSaved"
        @file-saved="notice = null"
        @ready="settle(true)"
        @load-failed="settle(false)"
      >
        <!-- Нижняя строка действий окна: слева импорт, экспорт и папка. -->
        <template #footer-start>
          <div class="cfgp__footer-start">
            <button type="button" class="settings-btn" data-tour="configs-import" :disabled="busy" :title="$t('#L_CFG_IMPORT')" :aria-label="$t('#L_CFG_IMPORT')" @click="importConfig">
              <i class="fas fa-file-import" aria-hidden="true"></i>
              <span>{{ $t("#L_CFG_IMPORT") }}</span>
            </button>
            <button type="button" class="settings-btn" data-tour="configs-export" :disabled="busy" :title="$t('#L_CFG_EXPORT')" :aria-label="$t('#L_CFG_EXPORT')" @click="exportConfig">
              <i class="fas fa-file-export" aria-hidden="true"></i>
              <span>{{ $t("#L_CFG_EXPORT") }}</span>
            </button>
            <button type="button" class="settings-btn" :title="$t('#L_CFG_OPEN_FOLDER')" :aria-label="$t('#L_CFG_OPEN_FOLDER')" @click="openFolder">
              <i class="fas fa-folder-open" aria-hidden="true"></i>
              <span>{{ $t("#L_CFG_OPEN_FOLDER") }}</span>
            </button>
          </div>
        </template>
      </GameSettingsModal>
    </div>

    <div v-if="unsaved" class="cfgp__scrim" @mousedown.self="answer('stay')">
      <div ref="dialog" class="cfgp__dialog" role="alertdialog" aria-modal="true" :aria-label="$t('#L_CFG_UNSAVED_TITLE')" @keydown="onDialogKey">
        <span class="cfgp__dialog-title">{{ $t("#L_CFG_UNSAVED_TITLE") }}</span>
        <div class="cfgp__dialog-actions">
          <button type="button" class="settings-btn" ref="stay" @click="answer('stay')">{{ $t("#L_CFG_UNSAVED_STAY") }}</button>
          <button type="button" class="settings-btn" @click="answer('discard')">{{ $t("#L_CFG_UNSAVED_DISCARD") }}</button>
          <button type="button" class="settings-btn primary" @click="answer('save')">{{ $t("#L_CFG_UNSAVED_SAVE") }}</button>
        </div>
      </div>
    </div>
  </section>
</template>

<script>
import GameSettingsModal from "./GameSettingsModal";
import { trapFocus } from "./focus-trap";
import prefsStore from "./launcher-prefs";
const bindPresets = require("../../../shared/bind-presets");
const cfgImport = require("../../../shared/cfg-import");
const { configChips } = require("./config-chips");

const { ipcRenderer } = window.hlsrElectron;

export default {
  name: "GameConfigsPanel",
  components: { GameSettingsModal },
  emits: ["saved", "help", "open-games", "import-started"],
  props: {
    // Установленные игры в порядке библиотеки: [{ id, name, title }].
    games: {
      type: Array,
      default: () => [],
    },
    initialGameId: {
      type: String,
      default: "",
    },
    // Растёт при каждом нажатии шестерёнки, даже если игра та же.
    openToken: {
      type: Number,
      default: 0,
    },
    // Раздел окна игры, которую открыли шестерёнкой или опросом ("configs"); пусто — первый.
    initialSection: {
      type: String,
      default: "",
    },
    // Игрок ответил в опросе «У меня есть свой конфиг»: запускаем импорт. Панель
    // принимает запрос один раз и сообщает об этом событием import-started.
    importRequested: {
      type: Boolean,
      default: false,
    },
  },
  data() {
    const first = this.games.find((game) => game.id === this.initialGameId) || this.games[0];
    return {
      gameId: first ? first.id : "",
      // Раздел, на котором откроется окно игры: initialSection у входа снаружи, при смене игры здесь — первый.
      openSection: this.initialSection,
      busy: false,
      // Итог последнего действия: { error, text, details, undo }.
      notice: null,
      // Вопрос о несохранённом: { resolve }.
      unsaved: null,
    };
  },
  // Вернулись на вкладку (keep-alive): файл игрока могли сохранить в «Персонализации».
  // Окно перечитывает его, только если в нём нет несохранённых правок (R18).
  activated() {
    const gs = this.settings();
    if (gs) gs.onWindowFocus();
  },
  // Ушли с вкладки при открытом вопросе: он закрывается ответом «Остаться».
  deactivated() {
    this.answer("stay");
    this.finishWait("cancelled");
  },
  computed: {
    chips() {
      return configChips(this.games);
    },
    gameTitle() {
      const game = this.games.find((item) => item.id === this.gameId);
      return game ? game.title : "";
    },
  },
  watch: {
    // Шестерёнка у игры открывает вкладку на ней.
    openToken() {
      if (!this.initialGameId) return;
      // Игра уже открыта: меняется только раздел, через обычный вопрос о несохранённом.
      const gs = this.settings();
      if (this.initialGameId === this.gameId && this.initialSection && gs) gs.selectSection(this.initialSection);
      else this.requestGame(this.initialGameId, this.initialSection);
    },
    // Окно игры пересоздаётся (:key): ожидание готовности старого окна отменяется.
    gameId() {
      this.finishWait("cancelled");
    },
    importRequested(value) {
      if (value) this.startImport();
    },
    // Выбранную игру удалили: вкладка переходит на первую из оставшихся.
    games(list) {
      if (!list.some((game) => game.id === this.gameId)) this.gameId = list.length ? list[0].id : "";
    },
  },
  mounted() {
    // Вкладку открыл опрос первого запуска: окно ещё читает настройки.
    if (this.importRequested) this.startImport();
  },
  methods: {
    settings() {
      return this.$refs.settings || null;
    },
    currentPageSection() {
      const gs = this.settings();
      return gs ? gs.activeSection : "";
    },
    // Обучение открывает раздел списка слева. При несохранённых правках не переключает и вопрос не открывает:
    // он встал бы под щитом обучения, куда не нажать. Тогда шаги без видимого якоря обучение пропускает.
    selectPageSection(id) {
      const gs = this.settings();
      if (!gs) return false;
      if (id !== gs.activeSection && (this.unsaved || this.dirty())) return false;
      return gs.selectSection(id);
    },
    // Опрос первого запуска: перейти на игру опроса, дождаться окна и начать импорт.
    async startImport() {
      this.$emit("import-started");
      if (this.initialGameId) await this.requestGame(this.initialGameId, this.initialSection);
      const id = this.gameId;
      if (id !== (this.initialGameId || id)) {
        this.notice = { error: true, text: this.$t("#L_CFG_IMPORT_NOT_READY") };
        return;
      }
      const state = await this.settingsReady();
      // Ушли с вкладки или сменили игру, пока ждали: импорт не начинаем и ничего не показываем.
      if (state === "cancelled" || this.gameId !== id) return;
      if (state === "ready") await this.importConfig();
      else this.notice = { error: true, text: this.$t("#L_CFG_IMPORT_NOT_READY") };
    },
    // "ready" | "failed" | "cancelled". Сначала читает состояние окна; событие ждёт только при "loading".
    async settingsReady() {
      await this.$nextTick();
      const gs = this.settings();
      if (!gs || gs.loadState === "failed") return "failed";
      if (gs.loadState === "ready") return "ready";
      return new Promise((resolve) => {
        this.readyWait = resolve;
      });
    },
    settle(ok) {
      this.finishWait(ok ? "ready" : "failed");
    },
    finishWait(state) {
      if (!this.readyWait) return;
      const resolve = this.readyWait;
      this.readyWait = null;
      resolve(state);
    },
    // Несохранённое на странице: черновик окна (общее «Сохранить» внизу) и раздел «Персонализация»
    // (своё «Сохранить»). part — "settings" или "personal": только эта часть (смена раздела слева).
    // Окно ещё читает настройки (draftSaved === null) — править там пока нечему.
    unsavedParts(part) {
      const gs = this.settings();
      return {
        settings: !!gs && part !== "personal" && gs.draftSaved !== null && gs.hasUnsaved(),
        personal: !!gs && part !== "settings" && !!gs.personalDirty,
      };
    },
    dirty(part) {
      const parts = this.unsavedParts(part);
      return parts.settings || parts.personal;
    },
    // true — можно идти дальше: правок нет, они сохранены или отброшены.
    async settleUnsaved(part) {
      const parts = this.unsavedParts(part);
      if (!parts.settings && !parts.personal) return true;
      const choice = await new Promise((resolve) => {
        this.unsaved = { resolve };
        this.$nextTick(() => this.$refs.stay && this.$refs.stay.focus());
      });
      this.unsaved = null;
      if (choice === "stay") return false;
      const gs = this.settings();
      const personal = gs.$refs.personal;
      if (choice === "save") {
        if (parts.personal) await personal.save();
        if (parts.settings) {
          // Несохранённый текст редактора cfg тоже сохраняется. Если он менял файл игрока, а в окне
          // есть и другие правки, «Сохранить» окна ответит «Файл изменён» — вопрос остаётся на игроке.
          if (gs.cfgFilesDirty && gs.$refs.cfgFiles) await gs.$refs.cfgFiles.save();
          await gs.submit();
        }
        // Флаг персонализации приходит событием dirty — после отрисовки.
        await this.$nextTick();
        return !this.dirty(part) && !(parts.settings && gs.fileChanged);
      }
      if (parts.personal) personal.revertGame();
      if (parts.settings) await gs.discardChanges();
      return true;
    },
    answer(choice) {
      if (this.unsaved) this.unsaved.resolve(choice);
    },
    onDialogKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        this.answer("stay");
      } else if (e.key === "Tab") {
        trapFocus(this.$refs.dialog, e);
      }
    },
    // section — раздел окна новой игры; пусто — тот же, что открыт сейчас (просьба владельца
    // 14.11: из «Клавиатуры» HL1 в «Клавиатуру» Ghosting). Нет такого у новой игры — первый в списке.
    async requestGame(id, section = "") {
      if (id === this.gameId || this.busy || this.unsaved || !this.games.some((game) => game.id === id)) return;
      const keep = section || this.currentPageSection();
      if (!(await this.settleUnsaved())) return;
      this.notice = null;
      this.openSection = keep;
      this.gameId = id;
    },
    // Импорт тем же выбором файлов, что и в окне (цепочка exec, файлы рядом), но
    // записывается сразу: вкладка потом перечитывает окно — файл игрока изменился.
    async importConfig() {
      const gs = this.settings();
      if (this.busy || !gs || !(await this.settleUnsaved())) return;
      this.busy = true;
      this.notice = null;
      try {
        const id = gs.bindsGameId || this.gameId;
        if (!(await gs.confirmImportIntro(bindPresets.engineFor(id)))) return;
        const picked = await ipcRenderer.invoke("player-config-import-pick", id, { multiple: true });
        if (!picked || picked.status === "canceled") return;
        if (picked.status !== "ok") {
          this.notice = { error: true, text: this.$t(picked.code === "BINDS_FILE_TOO_LARGE" ? "#L_KB_IMPORT_TOO_LARGE" : "#L_KB_IMPORT_FAILED") };
          return;
        }
        const engine = bindPresets.engineFor(id);
        const chosen = await gs.withCompanions(id, engine, picked.files || [picked], picked.dir);
        const { root, files: first } = cfgImport.rootOfPicked(chosen);
        const { files, missing } = await gs.pickImportChain(id, root, picked.dir, engine, false, first);
        const result = await ipcRenderer.invoke("player-config-import-apply", id, { root, files, missing });
        if (!result || result.status !== "ok") {
          this.notice = { error: true, text: gs.importErrorText(result) };
          return;
        }
        gs.importUndoable = true;
        await gs.reloadAfterChange();
        this.notice = this.importNotice(result, await this.ruleSummary(id));
      } catch (error) {
        this.notice = { error: true, text: this.$t("#L_KB_IMPORT_FAILED") };
      } finally {
        this.busy = false;
      }
    },
    // Любое сохранение в окне снимает итог импорта вместе с «Вернуть прежний» (R18).
    onSaved(settings) {
      this.notice = null;
      this.$emit("saved", settings);
    },
    // Что распознано, что не разобрано и какие файлы пропущены как небезопасные.
    // Итог проверки по правилам speedrun.com (только HL1): { errors, warnings } или null.
    async ruleSummary(id) {
      try {
        const check = await ipcRenderer.invoke("player-config-check", id, prefsStore.ruleCategoryOf());
        return check && check.status === "ok" ? { errors: check.stats.errors, warnings: check.stats.warnings } : null;
      } catch (error) {
        return null;
      }
    },
    showRules() {
      const gs = this.settings();
      if (gs) gs.showCheck();
    },
    importNotice(result, rules) {
      const summary = result.summary || {};
      const report = result.report || {};
      const unsafe = report.unsafe || [];
      // Пропущенный файл цепочки (отмена окна выбора), петля и повторы exec — не «всё успешно».
      const chainIssues = ["missing", "cycles", "duplicateFiles", "duplicates"].some((name) => (report[name] || []).length);
      const counts = this.$t("#L_CFG_IMPORT_DONE", String(summary.binds || 0), String(summary.cvars || 0),
        String(summary.aliases || 0), String(summary.files || 0));
      // Ничего не пропущено и правила не нарушены — просто «всё успешно» (просьба владельца 14.11).
      if (!summary.unknown && !unsafe.length && !chainIssues && !(rules && (rules.errors || rules.warnings))) {
        return { error: false, text: this.$t("#L_CFG_IMPORT_SUCCESS"), details: counts, undo: true, rules: false };
      }
      const parts = [counts];
      const gs = this.settings();
      if (chainIssues && gs) parts.push(...gs.importReport(Object.assign({ files: [], missing: [], cycles: [], duplicateFiles: [], duplicates: [] }, report, { files: [] })).short);
      if (summary.unknown) parts.push(this.$t("#L_CFG_IMPORT_UNKNOWN", String(summary.unknown)));
      if (unsafe.length) parts.push(this.$t("#L_CFG_IMPORT_UNSAFE", unsafe.join(", ")));
      if (rules) {
        parts.push(rules.errors || rules.warnings
          ? this.$t("#L_HC_IMPORT_LINE", String(rules.errors), String(rules.warnings))
          : this.$t("#L_HC_IMPORT_CLEAN"));
      }
      return {
        error: false, text: parts.join(" "), details: unsafe.length ? unsafe.join("\n") : null, undo: true,
        rules: !!rules && (rules.errors > 0 || rules.warnings > 0),
      };
    },
    async undoImport() {
      const gs = this.settings();
      if (this.busy || !gs || !(await this.settleUnsaved())) return;
      this.busy = true;
      try {
        const result = await ipcRenderer.invoke("player-config-import-undo", gs.bindsGameId || this.gameId);
        if (!result || result.status !== "ok") {
          this.notice = { error: true, text: gs.importErrorText(result) };
          return;
        }
        gs.importUndoable = false;
        await gs.reloadAfterChange();
        this.notice = { error: false, text: this.$t("#L_KB_IMPORT_REMOVED") };
      } catch (error) {
        this.notice = { error: true, text: this.$t("#L_KB_IMPORT_FAILED") };
      } finally {
        this.busy = false;
      }
    },
    async exportConfig() {
      const gs = this.settings();
      if (this.busy || !gs) return;
      this.busy = true;
      try {
        const result = await ipcRenderer.invoke("player-config-export", gs.bindsGameId || this.gameId);
        if (!result || result.status === "canceled") return;
        this.notice = result.status === "ok"
          ? { error: false, text: this.$t("#L_CFG_EXPORT_DONE", result.path) }
          : { error: true, text: this.$t("#L_CFG_EXPORT_FAILED") };
      } catch (error) {
        this.notice = { error: true, text: this.$t("#L_CFG_EXPORT_FAILED") };
      } finally {
        this.busy = false;
      }
    },
    openFolder() {
      const gs = this.settings();
      ipcRenderer.invoke("game-config-open-folder", this.gameId, gs ? gs.configBuild : null);
    },
  },
};
</script>

<style lang="scss" scoped>
@mixin icon-footer {
  &__footer-start { flex-wrap: nowrap; }
  &__footer-start .settings-btn { width: 48px; padding: 0; justify-content: center; }
  &__footer-start .settings-btn span { display: none; }
}

.cfgp {
  min-height: 0;
  padding: calc(var(--l-topbar-h) + 24px) var(--l-edge-pad) var(--l-edge-pad) var(--l-hero-pad);

  &__head {
    flex: none;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 16px 32px;
    margin-bottom: 16px;
  }

  &__title {
    margin: 0;
    color: #fff;
    font-size: 32px;
    font-weight: 700;
    line-height: 40px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  /* Выбор игры — «таблетки» на общей подложке, как вкладки персонализации. */
  &__games {
    display: flex;
    gap: 4px;
    padding: 4px;
    border-radius: var(--l-radius);
    background: var(--l-chip-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &__game {
    height: 36px;
    padding: 0 16px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.04em;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover:not(.active):not(:disabled):not(.is-soon) { color: var(--l-text); background: var(--l-chip-hover); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }

    &.active {
      color: var(--l-text);
      background: var(--accent-soft);
      box-shadow: inset 0 0 0 1px var(--accent-border);
    }

    &.is-soon { color: var(--l-text-faint); cursor: default; }
  }

  &__soon {
    margin-left: 8px;
    font-size: 11px;
    font-weight: 500;
    letter-spacing: 0;
  }

  &__footer-start {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  /* В узком окне (1280 px) три кнопки с подписями не оставляют места заметке и «Сохранить»: только значки, подписи — в title и aria-label.
     Выдвинутая колонка значков сужает страницу ещё на 160 px — тогда так же до окна в 1560 px. */
  /* Низкое окно (Full HD): заголовок мельче, чтобы выбор игры встал с ним в одну строку
     и разделу ниже досталось ~60 px высоты. */
  @media (max-height: 700px) {
    &__head { margin-bottom: 12px; }
    &__title { font-size: 24px; line-height: 32px; }
  }

  @media (max-width: 1400px) { @include icon-footer; }
  @media (max-width: 1560px) { .rail-open & { @include icon-footer; } }

  /* «?» — маленькая кнопка сразу за заголовком. */
  &__help {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    margin-left: -16px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--l-chip-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 13px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover { color: var(--l-text); background: var(--l-chip-hover); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
  }

  /* Итог импорта и других действий: тонкая строка между заголовком и вкладками. */
  &__notice {
    flex: none;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 12px;
    margin: 0 0 12px;
    padding: 8px 12px;
    border-radius: var(--l-radius-sm);
    background: var(--l-note-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.4;

    &.is-error { color: var(--l-level-error); }

    > span { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
  }


  &__empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
    color: var(--l-text-dim);
    font-size: 15px;

    p { margin: 0; }
  }

  /* Окно настроек занимает остаток страницы. */
  &__body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  &__scrim {
    position: absolute;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--l-modal-scrim);
  }

  &__dialog {
    display: flex;
    flex-direction: column;
    gap: 20px;
    max-width: min(440px, calc(100% - 48px));
    padding: 24px 28px;
    border-radius: var(--l-radius);
    background: var(--l-modal-bg);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6);
  }

  &__dialog-title {
    color: var(--l-text);
    font-size: 18px;
    font-weight: 700;
  }

  &__dialog-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
  }
}

/* Кнопки, как в окне настроек игры (там они в scoped-стиле другого компонента). */
.settings-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 48px;
  padding: 11px 19px;
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13.5px;
  font-weight: 600;
  line-height: 24px;
  cursor: pointer;
  transition: background var(--l-fast), border-color var(--l-fast), box-shadow var(--l-fast),
    color var(--l-fast), transform var(--l-fast);

  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
  &:active:not(:disabled) { transform: scale(0.97); }
  &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
  &:disabled { opacity: 0.45; cursor: not-allowed; }

  &.primary {
    background: var(--accent);
    border-color: transparent;
    color: var(--accent-contrast-text);

    &:hover:not(:disabled) {
      background: var(--accent-hover);
      box-shadow: 0 6px 20px color-mix(in srgb, var(--l-accent) 35%, transparent);
    }
    &:active:not(:disabled) { background: var(--accent-pressed); }
  }
}

/* Компактная кнопка в строке итога («Вернуть прежний конфиг»). */
.settings-btn.cfgp__compact {
  min-height: 32px;
  padding: 3px 12px;
  font-size: 12.5px;
  line-height: 20px;
}

.no-anim .settings-btn,
.no-anim .cfgp__help {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .settings-btn { transition: none; }
}
</style>
