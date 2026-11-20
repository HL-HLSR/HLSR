<template>
  <div class="gsc" :style="listWidth ? { '--gsc-list': listWidth + 'px' } : null">
    <aside class="gsc-list" :aria-label="$t('#L_SCR_LIST')">
      <label class="gsc-search">
        <i class="fas fa-search" aria-hidden="true"></i>
        <input
          v-model="query"
          type="search"
          class="gsc-search__input"
          spellcheck="false"
          :placeholder="$t('#L_SCR_SEARCH')"
          :aria-label="$t('#L_SCR_SEARCH')"
        />
      </label>
      <div class="gsc-list__scroll">
        <section v-for="chapter in chapters" :key="chapter.id" class="gsc-chapter">
          <h4 class="gsc-chapter__title">{{ chapter.title }}</h4>
          <button
            v-for="item in chapter.scripts"
            :key="item.id"
            type="button"
            class="gsc-item"
            :class="{ active: item.id === currentId }"
            :aria-current="item.id === currentId ? 'true' : 'false'"
            @click="open(item.id)"
          >
            <span class="gsc-mark" :class="item.mark ? 'mark-' + item.mark : 'mark-none'" aria-hidden="true"></span>
            <span class="gsc-item__title">{{ item.title }}</span>
            <span v-if="item.keys.length" class="gsc-item__keys">{{ item.keys.join(", ") }}</span>
          </button>
        </section>
        <p v-if="!chapters.length" class="gsc-empty">{{ $t("#L_SCR_NOTHING_FOUND") }}</p>
      </div>
    </aside>

    <PaneSplitter v-model="listWidth" storage-key="scripts:hl1-won" :min="220" :rest="400" label="#L_SPLIT_LIST" />

    <section v-if="current" class="gsc-detail">
      <div ref="scroll" class="gsc-detail__scroll">
        <header class="gsc-head">
          <div class="gsc-head__titles">
            <h3 class="gsc-head__title">{{ current.title }}</h3>
            <p class="gsc-head__meta">
              {{ chapterTitle }}<template v-if="current.section !== current.title"> · {{ current.section }}</template><template v-if="current.credit"> · {{ $t("#L_SCR_BY", current.credit) }}</template>
            </p>
          </div>
          <button v-if="current.save" type="button" class="gsc-btn" @click="openLink(current.save)">
            <i class="fas fa-download" aria-hidden="true"></i>{{ $t("#L_SCR_SAVE") }}
          </button>
          <button type="button" class="gsc-btn" @click="openLink(guideUrl)">
            <i class="fab fa-github" aria-hidden="true"></i>{{ $t("#L_SCR_GUIDE") }}
          </button>
        </header>

        <div class="gsc-video" :class="{ 'is-empty': !current.video }">
          <iframe
            v-if="current.video && playing"
            :key="current.id"
            class="gsc-video__frame"
            :src="embedUrl"
            :title="current.title"
            allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
            allowfullscreen
          ></iframe>
          <button
            v-else-if="current.video"
            type="button"
            class="gsc-video__poster"
            :style="{ backgroundImage: `url(${posterUrl})` }"
            :aria-label="$t('#L_SCR_PLAY')"
            @click="playing = true"
          >
            <span class="gsc-video__play"><i class="fas fa-play" aria-hidden="true"></i></span>
          </button>
          <p v-else class="gsc-video__none">
            <i class="fas fa-video-slash" aria-hidden="true"></i>{{ $t("#L_SCR_NO_VIDEO") }}
          </p>
        </div>
        <button v-if="current.video" type="button" class="gsc-link" @click="openLink(watchUrl)">
          {{ $t("#L_SCR_OPEN_YOUTUBE") }}
        </button>

        <section class="gsc-keys" :aria-label="$t('#L_SCR_KEYS')">
          <div class="gsc-keys__head">
            <span class="gsc-keys__title">{{ $t("#L_SCR_KEYS") }}</span>
            <span class="gsc-keys__spacer"></span>
            <button
              v-if="guideKeysMissing"
              type="button"
              class="gsc-btn"
              :disabled="!keyBinds"
              @click="assignGuideKeys"
            >
              <i class="fas fa-magic" aria-hidden="true"></i>{{ $t("#L_SCR_AS_GUIDE") }}
            </button>
          </div>
          <p v-if="!keyBinds" class="gsc-hint">{{ $t("#L_SCR_NOT_INSTALLED") }}</p>
          <div v-for="row in actionRows" :key="row.command" class="gsc-action">
            <code class="gsc-action__command" :title="row.command">{{ row.command }}</code>
            <span v-if="row.guideKey" class="gsc-action__guide">{{ $t("#L_SCR_GUIDE_KEY") }} <kbd class="gsc-kbd">{{ row.guideKeyName }}</kbd></span>
            <span class="gsc-action__spacer"></span>
            <span
              v-for="key in row.keys"
              :key="key.key"
              class="gsc-key"
              :class="{ 'gsc-key--config': key.fromConfig, 'gsc-key--part': !key.whole }"
              :title="key.title"
            >
              <i v-if="key.fromConfig" class="fas fa-file-import gsc-key__icon" aria-hidden="true"></i>
              {{ key.name }}
              <button
                v-if="key.whole"
                type="button"
                class="gsc-key__remove"
                :title="$t('#L_SCR_UNASSIGN', key.name)"
                :aria-label="$t('#L_SCR_UNASSIGN', key.name)"
                @click="unassign(key.key)"
              >
                <i class="fas fa-times" aria-hidden="true"></i>
              </button>
            </span>
            <button
              type="button"
              class="gsc-btn gsc-capture"
              :class="{ 'is-listening': capturing === row.command }"
              :disabled="!keyBinds"
              @click="startCapture(row.command, $event)"
              @mousedown="onCaptureMouse(row.command, $event)"
              @wheel="onCaptureWheel(row.command, $event)"
              @contextmenu.prevent
            >
              <template v-if="capturing === row.command">{{ $t("#L_SCR_PRESS_KEY") }}</template>
              <template v-else>
                <i class="fas fa-keyboard" aria-hidden="true"></i>{{ row.keys.length ? $t("#L_SCR_ADD_KEY") : $t("#L_SCR_ASSIGN") }}
              </template>
            </button>
          </div>
          <div v-if="conflict" class="gsc-conflict" role="alert">
            <p class="gsc-conflict__text">
              <template v-if="conflict.items.length === 1">{{
                $t("#L_SCR_BUSY_ONE", conflict.items[0].name, conflict.items[0].label)
              }}</template>
              <template v-else>{{ $t("#L_SCR_BUSY_MANY", conflict.items.map((item) => `${item.name} (${item.label})`).join(", ")) }}</template>
            </p>
            <button type="button" class="gsc-btn" @click="conflict = null">{{ $t("#L_CANCEL") }}</button>
            <button type="button" class="gsc-btn is-primary" @click="confirmConflict">{{ $t("#L_SCR_REPLACE") }}</button>
          </div>
          <p v-if="current.needsAutoAlias" class="gsc-hint">{{ $t("#L_SCR_AUTO_ALIAS") }}</p>
          <p v-if="usesSensitivity" class="gsc-hint">{{ $t("#L_SCR_SENSITIVITY", sensitivity) }}</p>
          <p v-if="scripted" class="gsc-hint">{{ $t("#L_SCR_SCRIPTED_ONLY") }}</p>
          <p v-if="ownAliases.length" class="gsc-hint">{{ $t("#L_SCR_OWN_ALIAS", ownAliases.join(", ")) }}</p>
        </section>
        <section class="gsc-desc" :aria-label="$t('#L_SCR_DESCRIPTION')">
          <div v-if="translation" class="gsc-keys__head">
            <span class="gsc-keys__title">{{ $t("#L_SCR_DESCRIPTION") }}</span>
            <span class="gsc-keys__spacer"></span>
            <button
              type="button"
              class="gsc-btn gsc-translate"
              :class="{ 'is-original': showOriginal }"
              :aria-pressed="showOriginal ? 'true' : 'false'"
              :title="$t(showOriginal ? '#L_SCR_SHOW_TRANSLATION' : '#L_SCR_SHOW_ORIGINAL')"
              :aria-label="$t(showOriginal ? '#L_SCR_SHOW_TRANSLATION' : '#L_SCR_SHOW_ORIGINAL')"
              @click="showOriginal = !showOriginal"
            >
              <i class="fas fa-language" aria-hidden="true"></i>
            </button>
          </div>
          <ul v-if="description.notes.length" class="gsc-notes">
            <li v-for="(note, index) in notesHtml" :key="index" v-html="note" @click="onLinkClick"></li>
          </ul>
          <div class="gsc-usage" v-html="usageHtml" @click="onLinkClick"></div>
        </section>
        <p class="gsc-source">{{ $t(translation && !showOriginal ? "#L_SCR_SOURCE_NOTE_TRANSLATED" : "#L_SCR_SOURCE_NOTE") }}</p>
      </div>

    </section>
  </div>
</template>

<script>
import { keyName, keyFromEvent } from "./keyboard-layout";
import { effectiveBinds, keysByPart, baseOf } from "./bind-state";
import PaneSplitter from "./PaneSplitter";
const binds = require("../../../shared/game-binds");
const bindPresets = require("../../../shared/bind-presets");
const guide = require("../../../shared/hl1-guide");
const markdown = require("../../../shared/guide-markdown");

const GAME_ID = "hl1-won";
const ENGINE = "goldsrc";
const SCRIPT_IDS = new Set(guide.SCRIPTS.map((script) => script.id));
const MD_OPTIONS = {
  imageBase: `https://raw.githubusercontent.com/yavats/hl1-scripts/${guide.SOURCE.commit}`,
  scriptIds: SCRIPT_IDS,
};

export default {
  name: "GameScripts",
  components: { PaneSplitter },
  emits: ["input"],
  props: {
    // Переназначения лаунчера: { KEY: { command, base } }.
    value: { type: Object, default: () => ({}) },
    // Что прочитано из cfg игры: { binds, aliases, cvars, files } или null.
    keyBinds: { type: Object, default: null },
    // Вкладка видна: видео останавливается, когда её закрывают.
    active: { type: Boolean, default: true },
  },
  data() {
    return {
      query: "",
      currentId: guide.SCRIPTS[0].id,
      playing: false,
      capturing: null,
      conflict: null,
      // Описание на английском, как в гайде, вместо перевода.
      showOriginal: false,
      // Ширина списка скриптов, px CSS (PaneSplitter); null — по умолчанию.
      listWidth: null,
    };
  },
  computed: {
    catalog() {
      return bindPresets.catalogFor(GAME_ID);
    },
    overrides() {
      return binds.sanitizeOverrides(this.value);
    },
    fileBinds() {
      return (this.keyBinds && this.keyBinds.binds) || {};
    },
    effective() {
      return effectiveBinds(this.fileBinds, this.overrides);
    },
    keysByCommand() {
      return keysByPart(this.effective, (this.keyBinds && this.keyBinds.aliases) || {});
    },
    chapters() {
      const needle = this.query.trim().toLowerCase();
      const result = [];
      for (const chapter of guide.CHAPTERS) {
        const scripts = guide.SCRIPTS.filter((script) => script.chapter === chapter.id)
          .filter((script) => !needle || this.matches(script, chapter, needle))
          .map((script) => ({
            id: script.id,
            title: script.title,
            mark: script.mark,
            keys: this.keysOfScript(script).map((key) => keyName(key, this.$t)),
          }));
        if (scripts.length) result.push({ id: chapter.id, title: chapter.title, scripts });
      }
      return result;
    },
    current() {
      return guide.scriptById(this.currentId);
    },
    chapterTitle() {
      const chapter = guide.CHAPTERS.find((item) => item.id === this.current.chapter);
      return chapter ? chapter.title : "";
    },
    guideUrl() {
      return `${guide.SOURCE.repo}#readme`;
    },
    posterUrl() {
      return `https://i.ytimg.com/vi/${this.current.video.id}/hqdefault.jpg`;
    },
    embedUrl() {
      const { id, start } = this.current.video;
      return `https://www.youtube-nocookie.com/embed/${id}?autoplay=1&rel=0&start=${start || 0}`;
    },
    watchUrl() {
      const { id, start } = this.current.video;
      return `https://www.youtube.com/watch?v=${id}${start ? `&t=${start}` : ""}`;
    },
    lang() {
      return Object.keys(this.$localization.locales)[this.$localization.lang] || "en";
    },
    // Перевод описания на язык интерфейса, если он есть для этой версии гайда.
    translation() {
      return guide.translationOf(this.current, this.lang);
    },
    description() {
      return this.translation && !this.showOriginal ? this.translation : this.current;
    },
    notesHtml() {
      return this.description.notes.map((note) => markdown.inline(note, MD_OPTIONS));
    },
    usageHtml() {
      return markdown.render(this.description.usage, MD_OPTIONS);
    },
    actionRows() {
      return this.current.actions.map((action) => {
        const guideKey = action.guideKey ? binds.normalizeKey(action.guideKey) : null;
        return {
          command: action.command,
          guideKey,
          guideKeyName: guideKey ? keyName(guideKey, this.$t) : "",
          keys: (this.keysByCommand[bindPresets.commandKey(action.command)] || [])
            .slice()
            .sort()
            .map((key) => this.keyChip(key, action.command)),
        };
      });
    },
    guideKeysMissing() {
      return this.actionRows.some((row) => row.guideKey && !row.keys.some((key) => key.key === row.guideKey));
    },
    // Alias скрипта, которые cfg игрока (обычно импортированный конфиг)
    // объявляет не так, как гайд.
    ownAliases() {
      const game = (this.keyBinds && this.keyBinds.aliases) || {};
      const norm = (value) => String(value || "").toLowerCase().replace(/\s*;\s*/g, ";").replace(/\s+/g, " ").trim();
      const names = [];
      for (const [name, value] of this.current.aliases) {
        if (guide.hasPlayerToken(value)) continue;
        if (!Object.prototype.hasOwnProperty.call(game, name)) continue;
        if (norm(game[name]) !== norm(value) && !names.includes(name)) names.push(name);
      }
      return names;
    },
    usesSensitivity() {
      return this.current.aliases.some(([, value]) => value.includes(guide.SENSITIVITY_TOKEN));
    },
    sensitivity() {
      const cvars = (this.keyBinds && this.keyBinds.cvars) || {};
      return guide.cleanSensitivity(cvars.sensitivity);
    },
    // У скрипта есть назначенное действие: напоминание о категориях Scripted.
    scripted() {
      return this.actionRows.some((row) => row.keys.length);
    },
  },
  watch: {
    currentId() {
      this.playing = false;
      this.stopCapture();
      this.conflict = null;
      if (this.$refs.scroll) this.$refs.scroll.scrollTop = 0;
    },
    active(value) {
      if (!value) {
        this.playing = false;
        this.stopCapture();
      }
    },
  },
  beforeUnmount() {
    this.stopCapture();
  },
  methods: {
    matches(script, chapter, needle) {
      const haystack = [script.title, script.section, script.credit, chapter.title]
        .concat(script.actions.map((action) => action.command))
        .join(" ")
        .toLowerCase();
      return needle.split(/\s+/).every((word) => haystack.includes(word));
    },
    // Клавиша действия: назначена целиком или частью составного бинда (тогда
    // её не снять отсюда — это чужой бинд), и откуда бинд — из
    // импортированного конфига или из cfg игры.
    keyChip(key, command) {
      const bound = this.effective[key] || {};
      const whole = bindPresets.commandKey(bound.command || "") === bindPresets.commandKey(command);
      const fromConfig = binds.isImportSource(bound.source, this.keyBinds && this.keyBinds.imported);
      const name = keyName(key, this.$t);
      let title = "";
      if (!whole) title = this.$t("#L_SCR_KEY_PART", name, bound.command || "");
      else if (fromConfig) title = this.$t("#L_SCR_KEY_CONFIG", name);
      return { key, name, whole, fromConfig, title: title || null };
    },
    keysOfScript(script) {
      const keys = new Set();
      for (const action of script.actions) {
        const preset = bindPresets.presetOf(this.catalog, action.command);
        // Общая команда (+bxt_tas_jumpbug) не значит, что назначен сам скрипт.
        if (preset && preset.guide !== script.id && !preset.aliases.length) continue;
        for (const key of this.keysByCommand[bindPresets.commandKey(action.command)] || []) keys.add(key);
      }
      return [...keys].sort();
    },
    open(id) {
      if (SCRIPT_IDS.has(id)) this.currentId = id;
    },
    openLink(url) {
      window.hlsrElectron.ipcRenderer.invoke("hl1-guide-open-link", url);
    },
    // Ссылки из описания: на другой скрипт гайда или наружу.
    onLinkClick(event) {
      const anchor = event.target.closest("a");
      if (!anchor) return;
      event.preventDefault();
      if (anchor.dataset.script) this.open(anchor.dataset.script);
      else if (anchor.dataset.href) this.openLink(anchor.dataset.href);
    },
    labelOf(command) {
      const preset = bindPresets.presetOf(this.catalog, command);
      if (!preset) return command;
      return preset.label[this.lang] || preset.label.en;
    },
    // Назначения { key: command }: свободные клавиши занимаются сразу, о
    // занятых спрашиваем.
    request(assignments) {
      const items = [];
      for (const [key, command] of Object.entries(assignments)) {
        const bound = this.effective[key];
        if (bound && bindPresets.commandKey(bound.command) !== bindPresets.commandKey(command)) {
          items.push({ key, name: keyName(key, this.$t), label: this.labelOf(bound.command) });
        }
      }
      if (items.length) this.conflict = { assignments, items };
      else this.apply(assignments);
    },
    confirmConflict() {
      if (!this.conflict) return;
      const { assignments } = this.conflict;
      this.conflict = null;
      this.apply(assignments);
    },
    apply(assignments) {
      const next = Object.assign({}, this.overrides);
      for (const [key, command] of Object.entries(assignments)) {
        next[key] = { command: binds.cleanCommand(command), base: baseOf(key, this.fileBinds, this.overrides) };
      }
      this.$emit("input", next);
    },
    assignGuideKeys() {
      const assignments = {};
      for (const row of this.actionRows) {
        if (row.guideKey && !row.keys.some((key) => key.key === row.guideKey)) assignments[row.guideKey] = row.command;
      }
      this.request(assignments);
    },
    // Снять скрипт с клавиши: вернуть то, что было на ней до лаунчера.
    unassign(key) {
      const next = Object.assign({}, this.overrides);
      const entry = next[key];
      if (entry) next[key] = { command: entry.base, base: entry.base };
      else next[key] = { command: "", base: this.fileBinds[key] ? this.fileBinds[key].command : "" };
      this.$emit("input", next);
    },
    startCapture(command, event) {
      // Клик по кнопке начинает ожидание, следующий клик по ней — кнопка мыши.
      if (this.capturing === command) return;
      event.stopPropagation();
      this.stopCapture();
      this.conflict = null;
      this.capturing = command;
      window.addEventListener("keydown", this.onCaptureKey, true);
      window.addEventListener("mousedown", this.onOutsideMouse, true);
    },
    stopCapture() {
      this.capturing = null;
      window.removeEventListener("keydown", this.onCaptureKey, true);
      window.removeEventListener("mousedown", this.onOutsideMouse, true);
    },
    finishCapture(key) {
      const command = this.capturing;
      this.stopCapture();
      if (key && command) this.request({ [key]: command });
    },
    onCaptureKey(event) {
      // Пока ждём клавишу, окно настроек не закрывается по Esc и Tab не
      // уводит фокус: Esc только отменяет ожидание.
      event.preventDefault();
      event.stopImmediatePropagation();
      if (event.code === "Escape") {
        this.stopCapture();
        return;
      }
      const key = keyFromEvent(event, ENGINE);
      if (key) this.finishCapture(key);
    },
    onCaptureMouse(command, event) {
      if (this.capturing !== command) return;
      event.preventDefault();
      event.stopPropagation();
      this.finishCapture(keyFromEvent(event, ENGINE));
    },
    onCaptureWheel(command, event) {
      if (this.capturing !== command) return;
      event.preventDefault();
      this.finishCapture(keyFromEvent(event, ENGINE));
    },
    onOutsideMouse(event) {
      if (event.target.closest && event.target.closest(".gsc-capture.is-listening")) return;
      this.stopCapture();
    },
  },
};
</script>

<style lang="scss" scoped>
.gsc {
  --gsc-green: #5fd068;
  --gsc-yellow: #f2cf4a;
  --gsc-orange: #ff9f43;
  --gsc-red: #ff6b6b;
  --gsc-purple: #b48cff;
  display: grid;
  // Между списком и скриптом — PaneSplitter (12 px) вместо зазора: длинные названия не влезали.
  // Ширина из окна побольше зажимается: списку не меньше 220 px, скрипту — 400 px.
  grid-template-columns: clamp(220px, var(--gsc-list, 28%), calc(100% - 412px)) 12px minmax(0, 1fr);
  gap: 0;
  height: 100%;
  min-height: 0;
  color: var(--l-text);
  color-scheme: dark;
}

.gsc-list,
.gsc-detail {
  display: flex;
  flex-direction: column;
  min-height: 0;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: var(--l-modal-bar);
  overflow: hidden;
}

.gsc-search {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 10px;
  padding: 0 10px;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text-faint);
  font-size: 11px;
  &:focus-within { border-color: var(--accent); }
  &__input {
    flex: 1;
    min-width: 0;
    padding: 7px 0;
    border: none;
    background: transparent;
    color: var(--l-text);
    font: inherit;
    font-size: 12px;
    &:focus { outline: none; }
  }
}

.gsc-list__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 0 6px 10px;
}

.gsc-chapter {
  & + & { margin-top: 8px; }
  &__title {
    margin: 0;
    padding: 6px 8px 4px;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }
}

.gsc-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  min-height: 30px;
  padding: 4px 8px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--l-text-dim);
  font: inherit;
  font-size: 12.5px;
  text-align: left;
  cursor: pointer;
  transition: background var(--l-fast), color var(--l-fast);
  &:hover { background: rgba(255, 255, 255, 0.05); color: var(--l-text); }
  &.active { background: var(--accent-soft); color: var(--l-text); }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
  &__title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  &__keys {
    flex: none;
    max-width: 40%;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--l-text);
    font-size: 10.5px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
}

// Цветная отметка из гайда: та же, что кружок перед названием в README.
.gsc-mark {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.16);
  &.mark-green { background: var(--gsc-green); }
  &.mark-yellow { background: var(--gsc-yellow); }
  &.mark-orange { background: var(--gsc-orange); }
  &.mark-red { background: var(--gsc-red); }
  &.mark-purple { background: var(--gsc-purple); }
}

.gsc-empty { margin: 16px 0 0; color: var(--l-text-faint); font-size: 12px; text-align: center; }

.gsc-detail__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 14px 16px 16px;
}

.gsc-head {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin-bottom: 12px;
  &__titles { flex: 1; min-width: 0; }
  &__title { margin: 0; font-size: 17px; font-weight: 600; line-height: 1.3; }
  &__meta { margin: 2px 0 0; color: var(--l-text-dim); font-size: 12px; }
}

.gsc-video {
  position: relative;
  width: 100%;
  max-width: 720px;
  aspect-ratio: 16 / 9;
  border-radius: var(--l-radius-sm);
  background: #000;
  overflow: hidden;
  &.is-empty {
    aspect-ratio: auto;
    max-width: none;
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }
  &__frame { position: absolute; inset: 0; width: 100%; height: 100%; border: 0; }
  &__poster {
    position: absolute;
    inset: 0;
    width: 100%;
    border: none;
    background: #000 center / cover no-repeat;
    cursor: pointer;
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
    &:hover .gsc-video__play { background: var(--accent); }
  }
  &__play {
    position: absolute;
    left: 50%;
    top: 50%;
    display: grid;
    place-items: center;
    width: 56px;
    height: 56px;
    margin: -28px 0 0 -28px;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
    font-size: 18px;
    transition: background var(--l-fast);
    i { margin-left: 3px; }
  }
  &__none {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 14px;
    color: var(--l-text-faint);
    font-size: 12px;
  }
}

.gsc-link {
  margin: 6px 0 0;
  padding: 0;
  border: none;
  background: none;
  color: var(--l-text-dim);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  &:hover { color: var(--l-text); text-decoration: underline; }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }
}

.gsc-notes {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin: 14px 0 0;
  padding: 0;
  list-style: none;
  li {
    padding: 3px 9px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 1.4;
  }
}

.gsc-usage,
.gsc-notes {
  :deep(code) {
    padding: 0 4px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--l-text);
    font-family: Consolas, monospace;
    font-size: 12px;
  }
  :deep(a) { color: var(--accent); text-decoration: none; &:hover { text-decoration: underline; } }
}

.gsc-usage {
  margin-top: 12px;
  color: var(--l-text);
  font-size: 13px;
  line-height: 1.55;
  overflow-wrap: anywhere;
  :deep(p) { margin: 0 0 8px; }
  :deep(ul), :deep(ol) { margin: 0 0 8px; padding-left: 20px; }
  :deep(li) { margin: 2px 0; }
  :deep(li > ul), :deep(li > ol) { margin: 2px 0 4px; }
  :deep(strong) { font-weight: 600; }
  :deep(pre) {
    margin: 0 0 8px;
    padding: 8px 10px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.3);
    overflow-x: auto;
    code { padding: 0; background: none; }
  }
  :deep(img) { display: block; max-width: min(100%, 480px); margin: 6px 0; border-radius: 6px; }
}

.gsc-desc { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--l-border); }
.gsc-desc > :first-child { margin-top: 0; }
.gsc-desc .gsc-keys__head + .gsc-notes { margin-top: 6px; }
.gsc-translate {
  justify-content: center;
  width: 30px;
  padding: 0;
  i { font-size: 15px; }
  &.is-original { background: var(--l-accent-soft); box-shadow: inset 0 0 0 1px var(--accent); color: var(--accent); }
}
.gsc-source { margin: 12px 0 0; color: var(--l-text-faint); font-size: 11.5px; }

.gsc-keys {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px solid var(--l-border);
  &__head { display: flex; align-items: center; gap: 8px; min-height: 30px; }
  &__title { font-size: 13px; font-weight: 600; }
  &__spacer { flex: 1; }
}

.gsc-kbd {
  display: inline-block;
  min-width: 18px;
  padding: 0 5px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.09);
  box-shadow: inset 0 -1px 0 rgba(0, 0, 0, 0.35);
  color: var(--l-text);
  font: inherit;
  font-weight: 600;
  text-align: center;
}

.gsc-action {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  min-height: 32px;
  &__command {
    min-width: 0;
    max-width: 50%;
    padding: 3px 8px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.06);
    font-family: Consolas, monospace;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  &__guide { color: var(--l-text-faint); font-size: 11.5px; }
  &__spacer { flex: 1; }
}

.gsc-key.gsc-key--config { background: rgba(var(--accent-rgb, 74, 134, 232), 0.18); }
.gsc-key.gsc-key--part { padding-right: 9px; font-style: italic; }
.gsc-key__icon { margin-right: 4px; font-size: 10px; opacity: 0.8; }

.gsc-key {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 2px 2px 2px 9px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.09);
  box-shadow: inset 0 -2px 0 rgba(0, 0, 0, 0.35);
  font-size: 12px;
  font-weight: 600;
  &__remove {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--l-text-dim);
    font-size: 10px;
    cursor: pointer;
    &:hover { background: rgba(255, 255, 255, 0.1); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }
  }
}

.gsc-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 30px;
  padding: 4px 10px;
  border: none;
  border-radius: var(--l-radius-sm);
  box-shadow: inset 0 0 0 1px var(--l-border-strong);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font: inherit;
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
  cursor: pointer;
  transition: background var(--l-fast), box-shadow var(--l-fast);
  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
  &:disabled { opacity: 0.45; cursor: default; }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
  i { font-size: 11px; }
  &.is-primary { background: var(--accent); box-shadow: none; color: var(--accent-contrast-text); &:hover { filter: brightness(1.1); background: var(--accent); } }
}

.gsc-capture.is-listening {
  background: var(--accent-soft);
  box-shadow: inset 0 0 0 1px var(--accent);
  animation: gsc-pulse 1.2s ease-in-out infinite;
}

@keyframes gsc-pulse {
  50% { box-shadow: inset 0 0 0 1px var(--accent), 0 0 0 3px var(--accent-soft); }
}

@media (prefers-reduced-motion: reduce) {
  .gsc-capture.is-listening { animation: none; }
}

.gsc-conflict {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  padding: 8px 10px;
  border-radius: var(--l-radius-sm);
  background: rgba(255, 159, 67, 0.1);
  box-shadow: inset 0 0 0 1px rgba(255, 159, 67, 0.35);
  &__text { flex: 1 1 240px; margin: 0; font-size: 12px; line-height: 1.45; }
}

.gsc-hint { margin: 0; color: var(--l-text-dim); font-size: 12px; line-height: 1.45; }
</style>
