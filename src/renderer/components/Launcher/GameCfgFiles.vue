<template>
  <section class="gx-pane cf" :aria-label="$t('#L_CF_TITLE')">
    <header class="gx-pane__head">
      <h3 class="gx-pane__title">{{ $t("#L_CF_TITLE") }}</h3>
      <button
        type="button"
        class="gx-btn gx-btn--icon"
        :disabled="loading"
        :title="$t('#L_CF_REFRESH')"
        :aria-label="$t('#L_CF_REFRESH')"
        @click="load"
      >
        <i class="fas fa-sync-alt" aria-hidden="true"></i>
      </button>
      <p class="gx-pane__note">{{ $t("#L_CF_NOTE") }}</p>
    </header>

    <div class="cf__body">
      <!-- Список cfg сборки: поиск по имени и пути. -->
      <div class="cf__list">
        <input
          v-model="filter"
          class="gx-input cf__search"
          type="search"
          :placeholder="$t('#L_CF_SEARCH')"
          :aria-label="$t('#L_CF_SEARCH')"
          autocomplete="off"
          spellcheck="false"
        />
        <p v-if="error" class="cf__state is-error" role="alert">{{ error }}</p>
        <p v-else-if="loading && !files.length" class="cf__state">{{ $t("#L_CF_LOADING") }}</p>
        <p v-else-if="!shownFiles.length" class="cf__state">{{ $t(filter ? "#L_CF_NOTHING_FOUND" : "#L_CF_EMPTY") }}</p>
        <ul v-else class="cf__files" role="listbox" :aria-label="$t('#L_CF_TITLE')">
          <li v-for="file in shownFiles" :key="file.path">
            <button
              type="button"
              role="option"
              class="cf__file"
              :class="{ 'is-active': current && current.path === file.path }"
              :aria-selected="current && current.path === file.path ? 'true' : 'false'"
              :title="file.path"
              @click="open(file.path)"
            >
              <span class="cf__file-name">
                <span class="cf__file-title">{{ baseName(file.path) }}</span>
                <span v-if="isPlayerFile(file.path)" class="cf__badge" :title="$t('#L_CF_PLAYER_FILE_HINT')">{{ $t("#L_CF_PLAYER_FILE") }}</span>
              </span>
              <span v-if="dirName(file.path)" class="cf__file-dir">{{ dirName(file.path) }}</span>
            </button>
          </li>
        </ul>
        <p v-if="truncated" class="cf__state">{{ $t("#L_CF_TRUNCATED") }}</p>
      </div>

      <div class="cf__editor" @keydown="onEditorKey">
        <template v-if="current">
          <div class="cf__bar">
            <code class="cf__path" :title="current.path">{{ current.path }}</code>
            <span v-if="dirty" class="cf__dirty">{{ $t("#L_CF_UNSAVED") }}</span>
            <button
              type="button"
              class="gx-btn gx-btn--icon cf__tool"
              :class="{ 'is-on': find.open }"
              :title="$t('#L_CF_FIND_BTN')"
              :aria-label="$t('#L_CF_FIND_BTN')"
              :aria-expanded="find.open ? 'true' : 'false'"
              @click="find.open ? closeFind() : openFind(false)"
            >
              <i class="fas fa-search" aria-hidden="true"></i>
            </button>
            <div class="cf__keys-wrap">
              <button
                ref="keysToggle"
                type="button"
                class="gx-btn gx-btn--icon cf__tool"
                :class="{ 'is-on': keysOpen }"
                :title="$t('#L_CF_KEYS')"
                :aria-label="$t('#L_CF_KEYS')"
                :aria-expanded="keysOpen ? 'true' : 'false'"
                aria-controls="cf-keys"
                @click="toggleKeys"
              >
                <i class="fas fa-keyboard" aria-hidden="true"></i>
              </button>
              <!-- Памятка — поверх окна: внутри панели редактора её обрезал край
                   панели. Садится под кнопкой, а если снизу не хватает места — над ней. -->
              <Teleport to=".launcher-root">
                <div
                  v-if="keysOpen"
                  id="cf-keys"
                  ref="keys"
                  class="cf__keys"
                  role="dialog"
                  :aria-label="$t('#L_CF_KEYS')"
                  :style="keysStyle"
                >
                  <p class="cf__keys-title">{{ $t("#L_CF_KEYS") }}</p>
                  <dl class="cf__keys-list">
                    <template v-for="item in shortcuts" :key="item.label">
                      <dt><kbd v-for="key in item.keys" :key="key">{{ key }}</kbd></dt>
                      <dd>{{ $t(item.label) }}</dd>
                    </template>
                  </dl>
                </div>
              </Teleport>
            </div>
          </div>

          <!-- Поиск и замена (Ctrl+F, Ctrl+H) и переход к строке (Ctrl+G). -->
          <div v-if="find.open" class="cf__find" role="search">
            <div class="cf__find-row">
              <button
                type="button"
                class="cf__find-mode"
                :title="$t('#L_CF_TOGGLE_REPLACE')"
                :aria-label="$t('#L_CF_TOGGLE_REPLACE')"
                :aria-expanded="find.replace ? 'true' : 'false'"
                @click="toggleReplace"
              >
                <i class="fas" :class="find.replace ? 'fa-chevron-down' : 'fa-chevron-right'" aria-hidden="true"></i>
              </button>
              <span class="cf__find-field" :class="{ 'is-error': findState.error }">
                <input
                  ref="findInput"
                  v-model="find.query"
                  class="gx-input cf__find-input"
                  type="text"
                  :placeholder="$t('#L_CF_FIND')"
                  :aria-label="$t('#L_CF_FIND')"
                  :aria-invalid="findState.error ? 'true' : null"
                  autocomplete="off"
                  spellcheck="false"
                  @keydown="onFindKey"
                />
                <button
                  v-for="flag in findFlags"
                  :key="flag.id"
                  type="button"
                  class="cf__flag"
                  :class="{ 'is-on': find[flag.id] }"
                  :title="$t(flag.label)"
                  :aria-label="$t(flag.label)"
                  :aria-pressed="find[flag.id] ? 'true' : 'false'"
                  @click="find[flag.id] = !find[flag.id]"
                >{{ flag.text }}</button>
              </span>
              <span class="cf__find-count" role="status">{{ findCountText }}</span>
              <button type="button" class="gx-btn gx-btn--icon" :disabled="!findState.matches.length" :title="$t('#L_CF_FIND_PREV')" :aria-label="$t('#L_CF_FIND_PREV')" @click="step(-1)">
                <i class="fas fa-arrow-up" aria-hidden="true"></i>
              </button>
              <button type="button" class="gx-btn gx-btn--icon" :disabled="!findState.matches.length" :title="$t('#L_CF_FIND_NEXT')" :aria-label="$t('#L_CF_FIND_NEXT')" @click="step(1)">
                <i class="fas fa-arrow-down" aria-hidden="true"></i>
              </button>
              <button type="button" class="gx-btn gx-btn--icon" :title="$t('#L_CF_CLOSE_FIND')" :aria-label="$t('#L_CF_CLOSE_FIND')" @click="closeFind">
                <i class="fas fa-times" aria-hidden="true"></i>
              </button>
            </div>
            <div v-if="find.replace" class="cf__find-row cf__find-row--replace">
              <input
                ref="replaceInput"
                v-model="find.replacement"
                class="gx-input cf__find-input"
                type="text"
                :placeholder="$t('#L_CF_REPLACE_WITH')"
                :aria-label="$t('#L_CF_REPLACE_WITH')"
                autocomplete="off"
                spellcheck="false"
                @keydown="onReplaceKey"
              />
              <button type="button" class="gx-btn" :disabled="!findState.matches.length || reading" @click="replaceCurrent">{{ $t("#L_CF_REPLACE") }}</button>
              <button type="button" class="gx-btn" :disabled="!findState.matches.length || reading" @click="replaceEverything">{{ $t("#L_CF_REPLACE_ALL") }}</button>
            </div>
          </div>
          <div v-if="gotoOpen" class="cf__find">
            <div class="cf__find-row">
              <label class="cf__goto-label" for="cf-goto">{{ $t("#L_CF_GOTO") }}</label>
              <input
                id="cf-goto"
                ref="gotoInput"
                v-model="gotoLine"
                class="gx-input cf__goto-input"
                type="number"
                min="1"
                :max="lines"
                :placeholder="'1–' + lines"
                autocomplete="off"
                @keydown="onGotoKey"
              />
              <button type="button" class="gx-btn" @click="goToLine">{{ $t("#L_CF_GOTO_OK") }}</button>
              <button type="button" class="gx-btn gx-btn--icon" :title="$t('#L_CF_CLOSE_FIND')" :aria-label="$t('#L_CF_CLOSE_FIND')" @click="closeGoto">
                <i class="fas fa-times" aria-hidden="true"></i>
              </button>
            </div>
          </div>

          <div class="cf__area">
            <div class="cf__gutter" aria-hidden="true">
              <pre class="cf__numbers" :style="{ transform: `translateY(${-scrollTop}px)` }">{{ lineNumbers }}</pre>
            </div>
            <textarea
              ref="text"
              v-model="text"
              class="cf__text"
              spellcheck="false"
              autocomplete="off"
              wrap="off"
              :aria-label="current.path"
              :disabled="reading"
              @keydown="onKey"
              @scroll="onScroll"
              @select="trackCaret"
              @click="trackCaret"
              @keyup="trackCaret"
            ></textarea>
          </div>
          <div class="cf__foot">
            <span class="cf__note" :class="{ 'is-error': notice && notice.error }" role="status">{{ footNote }}</span>
            <span class="cf__pos">{{ $t("#L_CF_POS", String(caret.line), String(caret.col)) }}</span>
            <button type="button" class="gx-btn" :disabled="!dirty || saving" @click="revert">
              <i class="fas fa-undo-alt" aria-hidden="true"></i>{{ $t("#L_CF_REVERT") }}
            </button>
            <button type="button" class="gx-btn cf__save" :disabled="!dirty || saving" :aria-busy="saving ? 'true' : null" @click="save">
              <i class="fas fa-save" aria-hidden="true"></i>{{ $t("#L_SAVE") }}
            </button>
          </div>
        </template>
        <p v-else class="cf__placeholder">{{ $t("#L_CF_PICK") }}</p>
      </div>
    </div>
  </section>
</template>

<script>
const { ipcRenderer } = window.hlsrElectron;
const textOps = require("../../../shared/cfg-text-ops");

const MOD = window.hlsrElectron.platform === "darwin" ? "⌘" : "Ctrl";

// Памятка горячих клавиш редактора.
const SHORTCUTS = [
  { keys: [MOD, "S"], label: "#L_CF_KEY_SAVE" },
  { keys: [MOD, "F"], label: "#L_CF_KEY_FIND" },
  { keys: [MOD, "H"], label: "#L_CF_KEY_REPLACE" },
  { keys: ["F3"], label: "#L_CF_KEY_NEXT" },
  { keys: ["Shift", "F3"], label: "#L_CF_KEY_PREV" },
  { keys: [MOD, "G"], label: "#L_CF_KEY_GOTO" },
  { keys: [MOD, "/"], label: "#L_CF_KEY_COMMENT" },
  { keys: [MOD, "D"], label: "#L_CF_KEY_DUPLICATE" },
  { keys: [MOD, "Shift", "K"], label: "#L_CF_KEY_DELETE" },
  { keys: ["Alt", "↑", "↓"], label: "#L_CF_KEY_MOVE" },
  { keys: ["Tab", "Shift", "Tab"], label: "#L_CF_KEY_INDENT" },
  { keys: [MOD, "Z"], label: "#L_CF_KEY_UNDO" },
  { keys: ["Esc"], label: "#L_CF_KEY_CLOSE" },
];

const FIND_FLAGS = [
  { id: "caseSensitive", text: "Aa", label: "#L_CF_MATCH_CASE" },
  { id: "wholeWord", text: "W", label: "#L_CF_WHOLE_WORD" },
  { id: "regex", text: ".*", label: "#L_CF_REGEX" },
];

// Редактор cfg-файлов сборки: любой cfg в папке игры правится прямо из
// лаунчера. Сохранение не ждёт закрытия игры — игрок сам выполнит exec.
// Правленый файл лаунчер потом не перезаписывает при обновлении и проверке.
export default {
  name: "GameCfgFiles",
  emits: ["dirty", "saved"],
  props: {
    gameId: { type: String, required: true },
    build: { type: String, default: null },
  },
  data() {
    return {
      // Основной файл игрока (путь как в списке) — помечается меткой; null — неизвестен.
      playerFile: null,
      files: [],
      truncated: false,
      loading: false,
      error: null,
      filter: "",
      current: null,
      text: "",
      reading: false,
      saving: false,
      running: false,
      notice: null,
      find: { open: false, replace: false, query: "", replacement: "", caseSensitive: false, wholeWord: false, regex: false },
      gotoOpen: false,
      gotoLine: "",
      keysOpen: false,
      keysStyle: {},
      scrollTop: 0,
      caret: { line: 1, col: 1 },
      selection: { start: 0, end: 0 },
      shortcuts: SHORTCUTS,
      findFlags: FIND_FLAGS,
    };
  },
  computed: {
    shownFiles() {
      const query = this.filter.trim().toLowerCase();
      const list = query ? this.files.filter((file) => file.path.toLowerCase().includes(query)) : this.files;
      // Основной файл игрока — первым (просьба владельца 14.11); остальные в прежнем порядке.
      return this.playerFile ? [...list].sort((a, b) => this.isPlayerFile(b.path) - this.isPlayerFile(a.path)) : list;
    },
    dirty() {
      return !!this.current && this.text !== this.current.content;
    },
    isGameConfig() {
      return !!this.current && /(^|\/)config\.cfg$/i.test(this.current.path);
    },
    lines() {
      return textOps.lineCount(this.text);
    },
    lineNumbers() {
      let out = "";
      for (let n = 1; n <= this.lines; n += 1) out += n === 1 ? "1" : "\n" + n;
      return out;
    },
    findOptions() {
      return { caseSensitive: this.find.caseSensitive, wholeWord: this.find.wholeWord, regex: this.find.regex };
    },
    findState() {
      if (!this.find.open || !this.find.query) return { matches: [], error: null };
      return textOps.findAll(this.text, this.find.query, this.findOptions);
    },
    // Номер совпадения, которое сейчас выделено (иначе ближайшее после курсора).
    findIndex() {
      const matches = this.findState.matches;
      if (!matches.length) return -1;
      const exact = matches.findIndex((m) => m.start === this.selection.start && m.end === this.selection.end);
      return exact !== -1 ? exact : textOps.nearestMatch(matches, this.selection.start);
    },
    findCountText() {
      if (this.findState.error) return this.$t("#L_CF_BAD_REGEX");
      if (!this.find.query) return "";
      const total = this.findState.matches.length;
      if (!total) return this.$t("#L_CF_NO_MATCHES");
      return this.$t("#L_CF_MATCHES", String(this.findIndex + 1), (this.findState.capped ? "≥" : "") + total);
    },
    footNote() {
      if (this.notice) return this.notice.text;
      if (this.running && this.isGameConfig) return this.$t("#L_CF_RUNNING_CONFIG");
      return this.$t("#L_CF_EXEC_HINT");
    },
  },
  watch: {
    dirty(value) {
      this.$emit("dirty", value);
    },
    gameId() {
      this.reset();
    },
    build() {
      this.reset();
    },
  },
  mounted() {
    this.loading_ = this.load();
    document.addEventListener("pointerdown", this.onOutsidePointer, true);
    window.addEventListener("resize", this.placeKeys);
  },
  beforeUnmount() {
    // Раздел закрыт без сохранения: текст пропал, флаг несохранённого не должен залипнуть.
    if (this.dirty) this.$emit("dirty", false);
    document.removeEventListener("pointerdown", this.onOutsidePointer, true);
    window.removeEventListener("resize", this.placeKeys);
  },
  methods: {
    isPlayerFile(relative) {
      return !!this.playerFile && relative.toLowerCase() === this.playerFile.toLowerCase();
    },
    baseName(file) {
      return file.slice(file.lastIndexOf("/") + 1);
    },
    dirName(file) {
      const at = file.lastIndexOf("/");
      return at > 0 ? file.slice(0, at) : "";
    },
    errorText(code) {
      const known = ["NOT_INSTALLED", "NOT_FOUND", "CHANGED", "TOO_LARGE", "ENCODING", "PATH_INVALID"];
      return this.$t(known.includes(code) ? "#L_CF_ERR_" + code : "#L_CF_ERR_IO");
    },
    confirmLeave() {
      return !this.dirty || window.confirm(this.$t("#L_CF_CONFIRM_DISCARD"));
    },
    reset() {
      this.current = null;
      this.text = "";
      this.notice = null;
      this.load();
    },
    async load() {
      this.loading = true;
      this.error = null;
      try {
        const result = await ipcRenderer.invoke("game-cfg-files-list", this.gameId, this.build);
        if (!result || result.status !== "ok") {
          this.files = [];
          this.error = this.errorText(result && result.code);
          return;
        }
        this.files = result.files;
        this.playerFile = typeof result.player === "string" ? result.player : null;
        this.truncated = !!result.truncated;
        this.running = !!result.running;
      } catch (error) {
        this.error = this.errorText();
      } finally {
        this.loading = false;
      }
    },
    async open(file) {
      if (this.current && this.current.path === file) return;
      if (!this.confirmLeave()) return;
      this.reading = true;
      this.notice = null;
      try {
        const result = await ipcRenderer.invoke("game-cfg-file-read", this.gameId, this.build, file);
        if (!result || result.status !== "ok") {
          this.notice = { error: true, text: this.errorText(result && result.code) };
          return;
        }
        this.running = !!result.running;
        this.current = { path: result.path, content: result.content, encoding: result.encoding, eol: result.eol, mtime: result.mtime };
        this.text = result.content;
        this.scrollTop = 0;
        this.selection = { start: 0, end: 0 };
        this.caret = { line: 1, col: 1 };
        this.$nextTick(() => {
          if (this.$refs.text) {
            this.$refs.text.scrollTop = 0;
            this.$refs.text.scrollLeft = 0;
            this.$refs.text.setSelectionRange(0, 0);
            if (!this.find.open) this.$refs.text.focus({ preventScroll: true });
          }
        });
      } catch (error) {
        this.notice = { error: true, text: this.errorText() };
      } finally {
        this.reading = false;
      }
    },
    // Открывает файл и ставит курсор на строку (проверка по правилам). Файла нет в списке сборки —
    // open сам скажет об ошибке. Правки в открытом другом файле: confirmLeave спросит, как при выборе в списке.
    async openAt(file, line) {
      await this.loading_;
      await this.open(file);
      if (!this.current || this.current.path !== file) return false;
      await this.$nextTick();
      if (line > 0 && line <= this.lines) {
        const start = textOps.lineStart(this.text, line);
        const end = this.text.indexOf("\n", start);
        this.reveal(start, end === -1 ? this.text.length : end);
      }
      if (this.$refs.text) this.$refs.text.focus({ preventScroll: true });
      return true;
    },
    revert() {
      if (!this.current) return;
      this.text = this.current.content;
      this.notice = null;
    },
    async save() {
      if (!this.dirty || this.saving) return;
      this.saving = true;
      this.notice = null;
      const saved = this.text;
      try {
        const meta = { encoding: this.current.encoding, eol: this.current.eol, mtime: this.current.mtime };
        const result = await ipcRenderer.invoke("game-cfg-file-write", this.gameId, this.build, this.current.path, saved, meta);
        if (!result || result.status !== "ok") {
          this.notice = { error: true, text: this.errorText(result && result.code) };
          return;
        }
        this.running = !!result.running;
        this.current = Object.assign({}, this.current, { content: saved, mtime: result.mtime });
        const name = this.baseName(this.current.path);
        this.notice = { text: this.$t(this.running ? "#L_CF_SAVED_RUNNING" : "#L_CF_SAVED", name, name) };
        // Окно настроек перечитает файл игрока: его могли править в этом редакторе.
        this.$emit("saved", { path: this.current.path });
      } catch (error) {
        this.notice = { error: true, text: this.errorText() };
      } finally {
        this.saving = false;
      }
    },
    // Правка через insertText: так её отменяет Ctrl+Z, как ввод с клавиатуры.
    applyEdit(edit, keepFocus) {
      const area = this.$refs.text;
      if (!area || !edit) return;
      const active = document.activeElement;
      area.focus({ preventScroll: true });
      area.setSelectionRange(edit.start, edit.end);
      const done = edit.insert
        ? document.execCommand("insertText", false, edit.insert)
        : edit.start === edit.end || document.execCommand("delete");
      if (!done) {
        area.setRangeText(edit.insert, edit.start, edit.end, "end");
        area.dispatchEvent(new Event("input", { bubbles: true }));
      }
      area.setSelectionRange(edit.selStart, edit.selEnd);
      this.trackCaret();
      if (keepFocus && active && active !== area && typeof active.focus === "function") active.focus({ preventScroll: true });
    },
    trackCaret() {
      const area = this.$refs.text;
      if (!area) return;
      this.selection = { start: area.selectionStart, end: area.selectionEnd };
      this.caret = textOps.lineCol(this.text, area.selectionEnd);
    },
    onScroll(event) {
      this.scrollTop = event.target.scrollTop;
    },
    // Выделить участок и прокрутить к нему, не забирая фокус у поля поиска.
    reveal(start, end) {
      const area = this.$refs.text;
      if (!area) return;
      area.setSelectionRange(start, end);
      this.selection = { start, end };
      this.caret = textOps.lineCol(this.text, end);
      const style = getComputedStyle(area);
      const lineHeight = parseFloat(style.lineHeight) || 19;
      const top = (this.caret.line - 1) * lineHeight;
      if (top < area.scrollTop || top + lineHeight > area.scrollTop + area.clientHeight)
        area.scrollTop = Math.max(0, top - area.clientHeight / 2);
      const charWidth = this.charWidth || (this.charWidth = this.measureChar(style));
      const from = textOps.lineCol(this.text, start).col - 1;
      const left = from * charWidth;
      const pad = parseFloat(style.paddingLeft) || 0;
      if (left < area.scrollLeft || left + pad * 2 > area.scrollLeft + area.clientWidth)
        area.scrollLeft = Math.max(0, left - area.clientWidth / 3);
      this.scrollTop = area.scrollTop;
    },
    measureChar(style) {
      const canvas = document.createElement("canvas").getContext("2d");
      canvas.font = `${style.fontSize} ${style.fontFamily}`;
      return canvas.measureText("0").width || 7.5;
    },
    selectedSingleLine() {
      const { start, end } = this.selection;
      const picked = this.text.slice(start, end);
      return picked && !picked.includes("\n") && picked.length <= 200 ? picked : null;
    },
    openFind(replace) {
      this.gotoOpen = false;
      this.trackCaret();
      const picked = this.selectedSingleLine();
      if (picked) this.find.query = this.find.regex ? picked.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") : picked;
      this.find.open = true;
      if (replace) this.find.replace = true;
      this.$nextTick(() => {
        const input = replace ? this.$refs.replaceInput : this.$refs.findInput;
        const target = replace && !this.find.query ? this.$refs.findInput : input;
        if (target) {
          target.focus();
          target.select();
        }
      });
    },
    closeFind() {
      this.find.open = false;
      this.find.replace = false;
      this.$nextTick(() => this.$refs.text && this.$refs.text.focus({ preventScroll: true }));
    },
    toggleReplace() {
      this.find.replace = !this.find.replace;
      if (this.find.replace) this.$nextTick(() => this.$refs.replaceInput && this.$refs.replaceInput.focus());
    },
    step(direction) {
      const matches = this.findState.matches;
      if (!matches.length) return;
      const { start, end } = this.selection;
      const index = direction < 0 ? textOps.nearestMatch(matches, start, true) : textOps.nearestMatch(matches, end > start ? end : start);
      const match = matches[index];
      this.reveal(match.start, match.end);
    },
    // Заменить выделенное совпадение и перейти к следующему.
    replaceCurrent() {
      const matches = this.findState.matches;
      const current = matches.find((m) => m.start === this.selection.start && m.end === this.selection.end);
      if (!current) return this.step(1);
      const edit = textOps.replaceOne(this.text, current, this.find.query, this.find.replacement, this.findOptions);
      this.applyEdit(Object.assign({}, edit, { selStart: edit.selEnd, selEnd: edit.selEnd }), true);
      this.$nextTick(() => this.step(1));
    },
    replaceEverything() {
      const result = textOps.replaceAll(this.text, this.find.query, this.find.replacement, this.findOptions);
      if (!result.count) return;
      const area = this.$refs.text;
      const keep = { top: area ? area.scrollTop : 0, caret: this.selection.start };
      const at = Math.min(keep.caret, result.edit.insert.length);
      this.applyEdit(Object.assign({}, result.edit, { selStart: at, selEnd: at }), true);
      if (area) area.scrollTop = keep.top;
      this.notice = { text: this.$t("#L_CF_REPLACED", String(result.count)) };
    },
    openGoto() {
      this.find.open = false;
      this.gotoOpen = true;
      this.gotoLine = String(this.caret.line);
      this.$nextTick(() => {
        if (this.$refs.gotoInput) {
          this.$refs.gotoInput.focus();
          this.$refs.gotoInput.select();
        }
      });
    },
    closeGoto() {
      this.gotoOpen = false;
      this.$nextTick(() => this.$refs.text && this.$refs.text.focus({ preventScroll: true }));
    },
    goToLine() {
      const line = Math.min(Math.max(1, parseInt(this.gotoLine, 10) || 1), this.lines);
      const at = textOps.lineStart(this.text, line);
      this.gotoOpen = false;
      this.$nextTick(() => {
        this.reveal(at, at);
        this.$refs.text.focus({ preventScroll: true });
      });
    },
    onFindKey(event) {
      if (event.key === "Enter") {
        event.preventDefault();
        this.step(event.shiftKey ? -1 : 1);
      }
    },
    onReplaceKey(event) {
      if (event.key !== "Enter") return;
      event.preventDefault();
      if ((event.ctrlKey || event.metaKey) && event.altKey) this.replaceEverything();
      else this.replaceCurrent();
    },
    onGotoKey(event) {
      if (event.key !== "Enter") return;
      event.preventDefault();
      this.goToLine();
    },
    // Сочетания всего редактора: и в тексте, и в полях поиска.
    onEditorKey(event) {
      const mod = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();
      if (mod && !event.altKey && (key === "f" || key === "h")) {
        event.preventDefault();
        this.openFind(key === "h");
      } else if (mod && !event.altKey && key === "g") {
        event.preventDefault();
        this.openGoto();
      } else if (event.key === "F3") {
        event.preventDefault();
        if (!this.find.open) this.openFind(false);
        else this.step(event.shiftKey ? -1 : 1);
      } else if (event.key === "Escape" && (this.find.open || this.gotoOpen || this.keysOpen)) {
        event.preventDefault();
        event.stopPropagation();
        if (this.keysOpen) {
          this.keysOpen = false;
          this.$refs.keysToggle && this.$refs.keysToggle.focus();
        } else if (this.gotoOpen) this.closeGoto();
        else this.closeFind();
      }
    },
    toggleKeys() {
      this.keysOpen = !this.keysOpen;
      if (this.keysOpen) this.$nextTick(this.placeKeys);
    },
    // Под кнопкой, если там хватает места; иначе туда, где места больше, а
    // список прокручивается в пределах окна.
    placeKeys() {
      const toggle = this.$refs.keysToggle;
      const popup = this.$refs.keys;
      if (!this.keysOpen || !toggle || !popup) return;
      const margin = 12;
      const gap = 6;
      const rect = toggle.getBoundingClientRect();
      const below = window.innerHeight - rect.bottom - gap - margin;
      const above = rect.top - gap - margin;
      const height = popup.scrollHeight;
      const right = Math.max(margin, window.innerWidth - rect.right) + "px";
      if (height <= below || below >= above) {
        this.keysStyle = { top: rect.bottom + gap + "px", right, maxHeight: below + "px" };
      } else {
        this.keysStyle = { bottom: window.innerHeight - rect.top + gap + "px", right, maxHeight: above + "px" };
      }
    },
    onOutsidePointer(event) {
      if (!this.keysOpen) return;
      const inside = (ref) => this.$refs[ref] && this.$refs[ref].contains(event.target);
      if (!inside("keys") && !inside("keysToggle")) this.keysOpen = false;
    },
    // Сочетания в самом тексте: сохранение, строки, отступы.
    onKey(event) {
      const mod = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();
      const area = event.target;
      const start = area.selectionStart;
      const end = area.selectionEnd;
      let edit = null;

      if (mod && !event.shiftKey && !event.altKey && key === "s") {
        event.preventDefault();
        this.save();
        return;
      }
      if (mod && !event.altKey && (event.key === "/" || event.code === "Slash")) edit = textOps.toggleComment(this.text, start, end);
      else if (mod && !event.shiftKey && !event.altKey && key === "d") edit = textOps.duplicateLines(this.text, start, end);
      else if (mod && event.shiftKey && !event.altKey && key === "k") edit = textOps.deleteLines(this.text, start, end);
      else if (event.altKey && !mod && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
        event.preventDefault();
        edit = textOps.moveLines(this.text, start, end, event.key === "ArrowUp" ? -1 : 1);
        if (!edit) return;
      } else if (event.key === "Tab" && !mod && !event.altKey) {
        const multiline = this.text.slice(start, end).includes("\n");
        if (event.shiftKey || multiline) edit = textOps.indentLines(this.text, start, end, event.shiftKey);
        else edit = { start, end, insert: "\t", selStart: start + 1, selEnd: start + 1 };
      }

      if (!edit) return;
      event.preventDefault();
      this.applyEdit(edit);
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

.cf {
  height: 100%;
}

.cf__body {
  display: grid;
  flex: 1;
  grid-template-columns: minmax(150px, 0.38fr) minmax(0, 1fr);
  min-height: 0;
}

.cf__list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  min-height: 0;
  padding: 10px;
  border-right: 1px solid var(--l-border);
}

.cf__search {
  flex: none;
  width: 100%;
}

.cf__files {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 0;
  list-style: none;
  overflow-y: auto;
  @include gx-scrollbar;
}

.cf__file {
  display: flex;
  flex-direction: column;
  gap: 1px;
  width: 100%;
  padding: 6px 9px;
  border: 0;
  border-radius: 7px;
  background: transparent;
  color: var(--l-text);
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: background var(--l-fast);

  &:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  &:focus-visible {
    outline: 2px solid var(--accent-focus);
    outline-offset: -2px;
  }
  &.is-active {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
}

.cf__file-name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-family: Consolas, monospace;
  font-size: 12.5px;
}

.cf__file-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

// Основной файл игрока: метка цвета акцента.
.cf__badge {
  flex: none;
  padding: 1px 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--accent) 22%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 55%, transparent);
  color: var(--l-text);
  font-family: Rubik, sans-serif;
  font-size: 10px;
  font-weight: 600;
  line-height: 14px;
}

.cf__file-dir {
  overflow: hidden;
  color: var(--l-text-faint);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cf__state {
  margin: 0;
  color: var(--l-text-faint);
  font-size: 12px;
  line-height: 1.4;

  &.is-error {
    color: #ff8a96;
  }
}

.cf__editor {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.cf__bar {
  display: flex;
  flex: none;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--l-border);
}

.cf__path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--l-text-dim);
  font-family: Consolas, monospace;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cf__dirty {
  flex: none;
  color: var(--accent);
  font-size: 11.5px;
  font-weight: 600;
}

.cf__tool {
  width: 30px;
  height: 30px;

  &.is-on {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--l-border));
    color: var(--l-text);
  }
}

.cf__keys-wrap {
  position: relative;
  flex: none;
}

/* Памятка горячих клавиш: поверх окна, место (top/bottom, right,
   max-height) считает placeKeys. Ширина — чтобы подписи не переносились. */
.cf__keys {
  @include gx-scrollbar;
  position: fixed;
  z-index: 1000;
  width: max-content;
  max-width: min(520px, calc(100vw - 24px));
  overflow-y: auto;
  padding: 12px 14px;
  border-radius: var(--l-radius);
  background: var(--l-modal-bg);
  box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
}

.cf__keys-title {
  margin: 0 0 8px;
  color: var(--l-text);
  font-size: 12.5px;
  font-weight: 600;
}

.cf__keys-list {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 5px 16px;
  align-items: center;
  margin: 0;

  dt {
    display: flex;
    gap: 3px;
    white-space: nowrap;
  }
  dd {
    margin: 0;
    color: var(--l-text-dim);
    font-size: 12px;
    white-space: nowrap;
  }
  kbd {
    min-width: 20px;
    padding: 2px 6px;
    border: 1px solid var(--l-border-strong);
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--l-text);
    font-family: inherit;
    font-size: 11px;
    line-height: 1.4;
    text-align: center;
  }
}

.cf__find {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: 6px;
  padding: 7px 12px;
  border-bottom: 1px solid var(--l-border);
  background: rgba(255, 255, 255, 0.02);
}

.cf__find-row {
  display: flex;
  align-items: center;
  gap: 6px;

  .gx-btn--icon {
    width: 28px;
    height: 28px;
  }
  &--replace {
    padding-left: 26px;
  }
}

.cf__find-mode {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 28px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--l-text-dim);
  cursor: pointer;

  &:hover {
    color: var(--l-text);
  }
  &:focus-visible {
    outline: 2px solid var(--accent-focus);
  }
  i {
    font-size: 10px;
  }
}

/* Флаги поиска стоят внутри поля, как в редакторах кода. */
.cf__find-field {
  position: relative;
  display: flex;
  flex: 1;
  min-width: 0;

  .cf__find-input {
    padding-right: 84px;
  }
  &.is-error .cf__find-input {
    border-color: #ff4d5e;
  }
}

.cf__find-input {
  flex: 1;
  width: 100%;
  height: 28px;
  font-family: Consolas, "DejaVu Sans Mono", monospace;
  font-size: 12.5px;
}

.cf__find-field .cf__flag {
  position: absolute;
  top: 3px;
  width: 24px;
  height: 22px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 5px;
  background: transparent;
  color: var(--l-text-faint);
  font-family: Consolas, "DejaVu Sans Mono", monospace;
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;

  &:nth-of-type(1) { right: 56px; }
  &:nth-of-type(2) { right: 30px; }
  &:nth-of-type(3) { right: 4px; }
  &:hover {
    color: var(--l-text);
  }
  &.is-on {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    color: var(--l-text);
  }
  &:focus-visible {
    outline: 2px solid var(--accent-focus);
    outline-offset: 0;
  }
}

.cf__find-count {
  flex: none;
  min-width: 82px;
  color: var(--l-text-faint);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  text-align: center;
  white-space: nowrap;
}

.cf__goto-label {
  flex: none;
  color: var(--l-text-dim);
  font-size: 12px;
}

.cf__goto-input {
  width: 110px;
  height: 28px;
  font-variant-numeric: tabular-nums;
}

.cf__area {
  position: relative;
  display: flex;
  flex: 1;
  min-height: 0;
  background: rgba(0, 0, 0, 0.18);
}

/* Номера строк: тот же шрифт и шаг, что у текста; прокручиваются вместе
   с ним сдвигом (textarea не переносит строки, высота строки постоянная). */
.cf__gutter {
  flex: none;
  overflow: hidden;
  border-right: 1px solid var(--l-border);
}

.cf__numbers {
  margin: 0;
  padding: 10px 8px 10px 10px;
  color: var(--l-text-faint);
  font-family: Consolas, "DejaVu Sans Mono", monospace;
  font-size: 12.5px;
  line-height: 1.55;
  text-align: right;
  user-select: none;
  will-change: transform;
}

.cf__pos {
  flex: none;
  color: var(--l-text-faint);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.cf__text {
  flex: 1;
  min-width: 0;
  min-height: 0;
  margin: 0;
  padding: 10px 12px;
  border: 0;
  background: transparent;
  color: var(--l-text);
  font-family: Consolas, "DejaVu Sans Mono", monospace;
  font-size: 12.5px;
  line-height: 1.55;
  tab-size: 4;
  white-space: pre;
  resize: none;
  overflow: auto;
  @include gx-scrollbar;

  /* Рамки нет: что поле в фокусе, и так видно по курсору. */
  &:focus,
  &:focus-visible {
    outline: none;
  }
  /* Найденное видно и когда фокус в поле поиска. */
  &::selection {
    background: color-mix(in srgb, var(--accent) 45%, transparent);
  }
}

.cf__foot {
  display: flex;
  flex: none;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-top: 1px solid var(--l-border);
}

.cf__note {
  flex: 1;
  min-width: 0;
  color: var(--l-text-faint);
  font-size: 12px;
  line-height: 1.35;

  &.is-error {
    color: #ff8a96;
  }
}

.cf__save:not(:disabled) {
  border-color: transparent;
  background: var(--accent);
  color: var(--accent-contrast-text, #fff);

  &:hover {
    border-color: transparent;
    background: color-mix(in srgb, var(--accent) 85%, #fff);
    color: var(--accent-contrast-text, #fff);
  }
}

.cf__placeholder {
  margin: auto;
  padding: 20px;
  color: var(--l-text-faint);
  font-size: 13px;
  text-align: center;
}
</style>
