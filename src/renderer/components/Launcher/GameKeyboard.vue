<template>
  <div class="kb">
    <section
      class="kb-board-pane"
      :style="{ '--kb-cols': layout.width, '--kb-h': boardHeight ? boardHeight + 'px' : null }"
      :aria-label="$t('#L_GS_S_KEYBOARD')"
    >
      <div class="kb-legend">
        <span v-for="item in legend" :key="item.id" class="kb-legend__item" :class="'tone-' + item.tone">
          <span class="kb-legend__swatch"></span>{{ item.title }}
        </span>
        <span class="kb-legend__item">
          <span class="kb-legend__dot"></span>{{ $t("#L_KB_LEGEND_LAUNCHER") }}
        </span>
        <span v-if="catalog.runModes && runMode === 'scriptless'" class="kb-legend__item">
          <span class="kb-legend__warn"></span>{{ $t("#L_KB_LEGEND_SCRIPT") }}
        </span>
        <span v-if="!keyBinds" class="kb-legend__note">{{ $t("#L_KB_NOT_INSTALLED") }}</span>
        <span class="kb-legend__spacer"></span>
        <template v-if="offBoard.length">
          <span class="kb-legend__note">{{ $t("#L_KB_OFF_BOARD") }}</span>
          <button
            v-for="item in offBoard"
            :key="item.key"
            type="button"
            class="kb-chip"
            :class="{ active: item.key === selected }"
            @mouseenter="hovered = item.key"
            @mouseleave="hovered = null"
            @click="select(item.key)"
          >
            <b>{{ item.name }}</b> {{ item.label }}
          </button>
        </template>
        <button
          type="button"
          class="kb-btn"
          :aria-pressed="full ? 'true' : 'false'"
          @click="$emit('update:full', !full)"
        >
          <i class="fas fa-keyboard" aria-hidden="true"></i>{{ $t(full ? "#L_KB_MAIN" : "#L_KB_FULL") }}
        </button>
      </div>

      <div ref="box" class="kb-board-box">
        <div
          ref="board"
          class="kb-board"
          role="group"
          :aria-label="$t('#L_GS_S_KEYBOARD')"
          @mouseleave="hovered = null"
        >
          <button
            v-for="cap in caps"
            :key="cap.id"
            type="button"
            class="kb-key"
            :class="cap.classes"
            :style="cap.style"
            :disabled="!cap.key"
            :aria-pressed="cap.key && cap.key === selected ? 'true' : 'false'"
            :aria-label="cap.aria"
            @mouseenter="hovered = cap.key"
            @focus="hovered = cap.key"
            @blur="hovered = null"
            @click="select(cap.key)"
          >
            <span class="kb-key__cap">{{ cap.text }}</span>
            <span v-if="cap.bind" class="kb-key__bind">{{ cap.bind }}</span>
            <span v-if="cap.script" class="kb-key__warn" aria-hidden="true"></span>
          </button>
        </div>
      </div>
    </section>

    <PaneSplitter
      v-model="boardHeight"
      :storage-key="'keyboard:' + gameId"
      orientation="horizontal"
      :min="162"
      :rest="190"
      :target="() => $refs.box"
      label="#L_SPLIT_KEYBOARD"
    />

    <div class="kb-bottom">
      <section class="kb-info" aria-live="polite">
        <template v-if="shown">
          <div class="kb-info__head">
            <span class="kb-info__key">{{ shown.name }}</span>
            <span v-if="shown.overridden" class="kb-badge is-launcher">{{ $t("#L_KB_SRC_LAUNCHER") }}</span>
            <span v-else-if="shown.source" class="kb-badge">{{ shown.source }}</span>
            <span class="kb-info__spacer"></span>
            <template v-if="isSelectedShown">
              <button
                type="button"
                class="kb-btn kb-btn--icon"
                :disabled="!shown.command"
                :title="$t('#L_KB_UNBIND')"
                :aria-label="$t('#L_KB_UNBIND')"
                @click="assign('')"
              >
                <i class="fas fa-eraser" aria-hidden="true"></i>
              </button>
              <button
                v-if="shown.overridden"
                type="button"
                class="kb-btn kb-btn--icon"
                :title="$t('#L_KB_REVERT')"
                :aria-label="$t('#L_KB_REVERT')"
                @click="revert"
              >
                <i class="fas fa-undo" aria-hidden="true"></i>
              </button>
            </template>
          </div>
          <p class="kb-info__bind" :class="'tone-' + shown.tone">
            {{ shown.label || $t("#L_KB_UNBOUND") }}
          </p>
          <!-- Бинд, который ставит cfg по нажатию другой клавиши (game-binds runtimeBinds). -->
          <p v-for="item in shown.runtime" :key="item.trigger + '|' + item.command" class="kb-info__note kb-info__runtime">
            {{ item.file ? $t("#L_KB_RUNTIME_FILE", item.trigger, item.file, item.label) : $t("#L_KB_RUNTIME", item.trigger, item.label) }}
          </p>
          <code v-if="shown.command && shown.label !== shown.command" class="kb-info__code">{{ shown.command }}</code>
          <p v-if="shown.note" class="kb-info__note">{{ shown.note }}</p>
          <p v-if="shown.scriptWarning" class="kb-info__warn">{{ $t("#L_KB_SCRIPT_WARNING") }}</p>
          <p v-if="shown.practice" class="kb-info__warn">{{ $t("#L_KB_PRACTICE_WARNING") }}</p>
          <p v-if="shown.alias" class="kb-info__note">
            {{ $t(shown.customAlias ? "#L_KB_ALIAS_CUSTOM" : "#L_KB_ALIAS") }} <code>{{ shown.alias }}</code>
          </p>
          <p v-if="shown.overridden" class="kb-info__note">
            {{ $t("#L_KB_WAS") }} {{ shown.baseLabel || $t("#L_KB_UNBOUND") }}
          </p>
          <p v-if="shown.otherKeys.length" class="kb-info__note">
            {{ $t("#L_KB_ALSO_ON") }} {{ shown.otherKeys.join(", ") }}
          </p>
          <p v-if="!isSelectedShown" class="kb-info__hint">{{ $t("#L_KB_CLICK_HINT") }}</p>
        </template>
        <p v-else class="kb-info__hint kb-info__hint--empty">{{ $t("#L_KB_PICK_KEY") }}</p>
      </section>

      <section class="kb-actions" :class="{ 'is-idle': !selected }">
        <header class="kb-actions__head">
          <span class="kb-actions__title">{{
            selected ? $t("#L_KB_ASSIGN_TO", selectedName) : $t("#L_KB_ASSIGN")
          }}</span>
          <div v-if="catalog.runModes" class="kb-modes" role="radiogroup" :aria-label="$t('#L_KB_RUN_MODE')">
            <button
              v-for="mode in runModes"
              :key="mode.id"
              type="button"
              role="radio"
              class="kb-modes__item"
              :class="{ active: runMode === mode.id }"
              :aria-checked="runMode === mode.id ? 'true' : 'false'"
              :title="$t(mode.hint)"
              @click="setRunMode(mode.id)"
            >
              {{ mode.title }}
            </button>
          </div>
          <form class="kb-custom" @submit.prevent="assignCustom">
            <input
              v-model="custom"
              type="text"
              class="kb-custom__input"
              :class="{ invalid: customError }"
              :disabled="!selected"
              spellcheck="false"
              :maxlength="maxCommand"
              :placeholder="$t('#L_KB_CUSTOM_PLACEHOLDER')"
              :aria-label="$t('#L_KB_CUSTOM')"
              @input="customError = null"
            />
            <button type="submit" class="kb-btn" :disabled="!selected || !custom.trim()">
              {{ $t("#L_KB_CUSTOM_APPLY") }}
            </button>
          </form>
        </header>
        <div class="kb-filter">
          <label class="kb-search">
            <i class="fas fa-search" aria-hidden="true"></i>
            <input
              v-model="query"
              type="search"
              class="kb-search__input"
              spellcheck="false"
              :placeholder="$t('#L_KB_SEARCH')"
              :aria-label="$t('#L_KB_SEARCH')"
            />
          </label>
          <div class="kb-cats" role="tablist" :aria-label="$t('#L_KB_CATEGORIES')" @wheel="scrollCats">
            <button
              type="button"
              role="tab"
              class="kb-cats__item"
              :class="{ active: !category }"
              :aria-selected="!category ? 'true' : 'false'"
              @click="category = null"
            >
              {{ $t("#L_KB_ALL") }}
            </button>
            <button
              v-for="item in listedCategories"
              :key="item.id"
              type="button"
              role="tab"
              class="kb-cats__item"
              :class="['tone-' + item.tone, { active: category === item.id }]"
              :aria-selected="category === item.id ? 'true' : 'false'"
              @click="category = category === item.id ? null : item.id"
            >
              {{ text(item.title) }}
            </button>
          </div>
        </div>
        <p v-if="customError" class="kb-actions__error" role="alert">{{ customError }}</p>
        <div class="kb-actions__list">
          <div v-for="group in groups" :key="group.id" class="kb-group" :class="'tone-' + group.tone">
            <div class="kb-group__items">
              <span class="kb-group__title">{{ group.title }}</span>
              <button
                v-for="action in group.actions"
                :key="action.command"
                type="button"
                class="kb-chip"
                :class="{ active: shownSelected && shownSelected.key && sameCommand(shownSelected.command, action.command) }"
                :disabled="!selected"
                :title="action.title"
                @click="assign(action.command)"
              >
                {{ action.label }}
                <span v-if="action.scripted" class="kb-chip__tag">{{ $t("#L_KB_TAG_SCRIPT") }}</span>
                <span v-if="action.practice" class="kb-chip__tag is-practice">{{ $t("#L_KB_TAG_PRACTICE") }}</span>
                <span v-if="action.keys.length" class="kb-chip__keys">{{ action.keys.join(", ") }}</span>
              </button>
            </div>
          </div>
          <p v-if="!groups.length" class="kb-actions__empty">{{ $t("#L_KB_NOTHING_FOUND") }}</p>
        </div>
      </section>
    </div>
  </div>
</template>

<script>
import { BOARD_HEIGHT, layoutFor, keyName } from "./keyboard-layout";
import { effectiveBinds, keysByCommand, baseOf } from "./bind-state";
import PaneSplitter from "./PaneSplitter";
const binds = require("../../../shared/game-binds");
const bindPresets = require("../../../shared/bind-presets");
const { ipcRenderer } = window.hlsrElectron;

const RUN_MODE_KEY = "hlsr.keyboard.runMode";
const EMPTY_CATALOG = { engine: "goldsrc", categories: [], presets: [], byCommand: {}, runModes: false };

function readRunMode() {
  try {
    const value = window.localStorage.getItem(RUN_MODE_KEY);
    return value === "scripted" ? "scripted" : "scriptless";
  } catch (error) {
    return "scriptless";
  }
}

export default {
  name: "GameKeyboard",
  components: { PaneSplitter },
  emits: ["input", "update:full"],
  props: {
    // Сборка, чьи заготовки показывать (hl1-won, ghosting-renewed, hl2-4260…).
    gameId: { type: String, required: true },
    // Переназначения лаунчера: { KEY: { command, base } }.
    value: { type: Object, default: () => ({}) },
    // Что прочитано из cfg игры: { binds, aliases, files } или null.
    keyBinds: { type: Object, default: null },
    // Вся клавиатура или только основной блок (запоминает родитель).
    full: { type: Boolean, default: true },
    // Сохранённое (не черновик): переназначения последнего сохранения и cfg с
    // диска — для «Пианино». null — сохранённое не известно.
    savedValue: { type: Object, default: null },
    savedKeyBinds: { type: Object, default: null },
  },
  data() {
    return {
      hovered: null,
      selected: null,
      custom: "",
      customError: null,
      maxCommand: binds.MAX_COMMAND,
      query: "",
      category: null,
      runMode: readRunMode(),
      // Высота схемы, px CSS (PaneSplitter); null — по умолчанию.
      boardHeight: null,
    };
  },
  computed: {
    catalog() {
      return bindPresets.catalogFor(this.gameId) || EMPTY_CATALOG;
    },
    layout() {
      return layoutFor(this.catalog.engine, { full: this.full });
    },
    lang() {
      return Object.keys(this.$localization.locales)[this.$localization.lang] || "en";
    },
    runModes() {
      return [
        { id: "scriptless", title: "Scriptless", hint: "#L_KB_SCRIPTLESS_HINT" },
        { id: "scripted", title: "Scripted", hint: "#L_KB_SCRIPTED_HINT" },
      ];
    },
    overrides() {
      return binds.sanitizeOverrides(this.value);
    },
    fileBinds() {
      return (this.keyBinds && this.keyBinds.binds) || {};
    },
    aliases() {
      return (this.keyBinds && this.keyBinds.aliases) || {};
    },
    // Бинды, которые cfg ставит по нажатию другой клавиши: { key: [{ command, file, trigger }] }.
    runtime() {
      return (this.keyBinds && this.keyBinds.runtime) || {};
    },
    // Итог для каждой клавиши: то, что будет в игре после запуска.
    effective() {
      return effectiveBinds(this.fileBinds, this.overrides);
    },
    keysByCommand() {
      return keysByCommand(this.effective);
    },
    // «Пианино»: в сохранённых биндах занята каждая клавиша полной клавиатуры
    // и каждая кнопка мыши с её схемы (MOUSE1–5, колесо вверх и вниз), какой
    // бы вид ни был на экране.
    fullKeys() {
      return layoutFor(this.catalog.engine, { full: true }).keys.filter((cap) => cap.key);
    },
    savedEffective() {
      return effectiveBinds((this.savedKeyBinds && this.savedKeyBinds.binds) || {}, this.savedValue || {});
    },
    everyKeyBound() {
      return this.savedValue !== null && this.fullKeys.length > 0 && this.fullKeys.every((cap) => !!this.savedEffective[cap.key]);
    },
    // Категории, заготовки которых предлагаются в списке: скрипты гайда HL1
    // назначаются во вкладке «Скрипты».
    listedCategories() {
      return this.catalog.categories.filter((item) =>
        this.catalog.presets.some((preset) => preset.category === item.id && preset.listed)
      );
    },
    tones() {
      const map = {};
      for (const item of this.catalog.categories) map[item.id] = item.tone;
      return map;
    },
    legend() {
      // Цвета категорий видны в фильтре над заготовками.
      const result = [{ id: "other", tone: "other", title: this.$t("#L_KB_G_OTHER") }];
      if (Object.values(this.effective).some((bound) => bound && this.playerAlias(bound.command)))
        result.push({ id: "alias", tone: "alias", title: this.$t("#L_KB_LEGEND_ALIAS") });
      if (this.layout.keys.some((cap) => cap.key && this.runtime[cap.key]))
        result.push({ id: "runtime", tone: "runtime", title: this.$t("#L_KB_LEGEND_RUNTIME") });
      return result;
    },
    // alias игрока по имени без учёта регистра, как в движке.
    aliasNames() {
      const map = new Map();
      for (const [name, value] of Object.entries(this.aliases)) map.set(name.toLowerCase(), value);
      return map;
    },
    scriptless() {
      return this.catalog.runModes && this.runMode === "scriptless";
    },
    caps() {
      const u = 100 / this.layout.width;
      const v = 100 / BOARD_HEIGHT;
      return this.layout.keys.map((cap) => {
        const bound = cap.key ? this.effective[cap.key] : null;
        // Пустая на старте клавиша, которую cfg назначит по нажатию другой: её бинд — своим цветом.
        const later = cap.key && !bound && this.runtime[cap.key] ? this.runtime[cap.key][0] : null;
        const tone = bound ? this.toneOf(bound.command) : later ? "runtime" : null;
        const text = cap.label.startsWith("#") ? this.$t(cap.label) : cap.label;
        return {
          id: cap.id,
          key: cap.key,
          text,
          bind: bound ? this.labelOf(bound.command) : later ? this.labelOf(later.command) : "",
          script: !!bound && this.scriptless && bindPresets.isScripted(this.catalog, bound.command, this.aliases),
          aria: cap.key
            ? `${keyName(cap.key, this.$t)}: ${bound ? this.labelOf(bound.command) : later ? this.labelOf(later.command) : this.$t("#L_KB_UNBOUND")}`
            : text,
          classes: {
            ["tone-" + tone]: !!tone,
            "is-bound": !!bound,
            "is-runtime": !!cap.key && !!this.runtime[cap.key],
            "is-wide": cap.w >= 2,
            "is-launcher": (bound && bound.overridden) || (!!cap.key && this.isUnboundByLauncher(cap.key)),
            "is-selected": cap.key && cap.key === this.selected,
            "is-dead": !cap.key,
          },
          style: {
            left: `${cap.x * u}%`,
            top: `${cap.y * v}%`,
            width: `${cap.w * u}%`,
            height: `${cap.h * v}%`,
          },
        };
      });
    },
    // Забинженные клавиши, которых нет на схеме: «~», «+» и т. п.
    offBoard() {
      return Object.keys(this.effective)
        .filter((key) => !layoutFor(this.catalog.engine).boardKeys.has(key))
        .sort()
        .map((key) => ({ key, name: keyName(key, this.$t), label: this.labelOf(this.effective[key].command) }));
    },
    groups() {
      const needle = this.query.trim().toLowerCase();
      const result = [];
      for (const item of this.catalog.categories) {
        if (this.category && this.category !== item.id) continue;
        const actions = this.catalog.presets
          .filter((preset) => preset.category === item.id && preset.listed)
          .filter((preset) => !(this.scriptless && preset.scripted))
          .filter((preset) => !needle || this.matches(preset, needle))
          .map((preset) => {
            const keys = (this.keysByCommand[bindPresets.commandKey(preset.command)] || []).map((key) =>
              keyName(key, this.$t)
            );
            const note = preset.note ? this.text(preset.note) : "";
            return {
              command: preset.command,
              label: this.text(preset.label),
              scripted: this.catalog.runModes && preset.scripted,
              practice: preset.practice,
              keys,
              title: [preset.command, note, keys.join(", ")].filter(Boolean).join(" · "),
            };
          });
        if (actions.length) result.push({ id: item.id, tone: item.tone, title: this.text(item.title), actions });
      }
      return result;
    },
    shown() {
      const key = this.hovered || this.selected;
      return key ? this.describe(key) : null;
    },
    shownSelected() {
      return this.selected ? this.describe(this.selected) : null;
    },
    isSelectedShown() {
      return !!this.selected && (!this.hovered || this.hovered === this.selected);
    },
    selectedName() {
      return this.selected ? keyName(this.selected, this.$t) : "";
    },
    capTexts() {
      return this.caps.map((cap) => cap.text).join("\n");
    },
  },
  watch: {
    gameId() {
      this.selected = null;
      this.hovered = null;
      this.category = null;
      this.query = "";
    },
    capTexts() {
      this.$nextTick(this.fitCaps);
    },
    // immediate: всё уже занято к открытию клавиатуры — тоже засчитывается.
    everyKeyBound: { handler: "unlockPiano", immediate: true },
  },
  mounted() {
    // Схема меняет ширину вместе с окном, а вкладка до первого показа скрыта:
    // наблюдатель подгонит подписи, когда у клавиш появится размер. Подгонка
    // в следующем кадре: правка размеров прямо в колбэке — «ResizeObserver loop».
    this.boardObserver = new ResizeObserver(() => {
      cancelAnimationFrame(this.fitFrame);
      this.fitFrame = requestAnimationFrame(() => this.fitCaps());
    });
    this.boardObserver.observe(this.$refs.board);
  },
  beforeUnmount() {
    if (this.boardObserver) this.boardObserver.disconnect();
    cancelAnimationFrame(this.fitFrame);
    this.boardObserver = null;
  },
  methods: {
    // «Pause», «Home» и «Wheel ↑» шире клавиши в 1u: такая подпись мельчает
    // ровно до ширины клавиши, а не обрезается её рамкой. Сначала все
    // подписи сбрасываются и замеряются, потом меняются, чтобы не
    // пересчитывать раскладку на каждой клавише.
    fitCaps() {
      const board = this.$refs.board;
      if (!board || !board.clientWidth) return;
      const labels = Array.from(board.querySelectorAll(".kb-key__cap"));
      for (const label of labels) label.style.fontSize = "";
      const sizes = labels.map((label) => {
        const key = label.parentElement;
        const style = getComputedStyle(key);
        const room = key.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
        const width = label.scrollWidth;
        if (width <= room) return null;
        return Math.floor(parseFloat(getComputedStyle(label).fontSize) * (room / width) * 10) / 10;
      });
      labels.forEach((label, i) => {
        if (sizes[i]) label.style.fontSize = sizes[i] + "px";
      });
    },
    unlockPiano(full) {
      if (full) ipcRenderer.invoke("runner-secret-unlock", "piano").catch(() => {});
    },
    // Колесо мыши листает строку категорий вбок, если она не влезла.
    scrollCats(event) {
      const row = event.currentTarget;
      if (row.scrollWidth <= row.clientWidth || !event.deltaY) return;
      event.preventDefault();
      row.scrollLeft += event.deltaY;
    },
    // Подписи каталога — { ru, en }, подписи интерфейса — ключи локализации.
    text(value) {
      if (!value) return "";
      if (typeof value === "string") return this.$t(value);
      return value[this.lang] || value.en || "";
    },
    matches(preset, needle) {
      const haystack = [preset.command, preset.label.ru, preset.label.en]
        .concat(preset.note ? [preset.note.ru, preset.note.en] : [])
        .join(" ")
        .toLowerCase();
      return needle.split(/\s+/).every((word) => haystack.includes(word));
    },
    sameCommand(a, b) {
      return bindPresets.commandKey(a) === bindPresets.commandKey(b);
    },
    setRunMode(mode) {
      this.runMode = mode;
      try {
        window.localStorage.setItem(RUN_MODE_KEY, mode);
      } catch (error) {
        // Режим просто не запомнится.
      }
    },
    toneOf(command) {
      if (this.playerAlias(command)) return "alias";
      const preset = bindPresets.presetOf(this.catalog, command);
      if (preset) return this.tones[preset.category] || "other";
      return "other";
    },
    aliasOf(word) {
      const value = this.aliasNames.get(String(word).toLowerCase());
      return value === undefined ? null : value;
    },
    // Клавиша назначена на alias из cfg игрока (или импорта): { name, value }.
    // Имя может совпадать со скриптом из заготовок (labo2, gonch…), но в
    // игре работает alias игрока, поэтому показывается он. Заготовкой бинд
    // остаётся, только если alias объявлен в точности как в ней.
    playerAlias(command) {
      if (!command || /[\s;]/.test(command)) return null;
      const value = this.aliasOf(command);
      if (value === null) return null;
      const preset = bindPresets.presetOf(this.catalog, command);
      const same = (a, b) => String(a).replace(/\s+/g, " ").trim().toLowerCase() === String(b).replace(/\s+/g, " ").trim().toLowerCase();
      if (preset && preset.aliases.some(([name, text]) => name.toLowerCase() === command.toLowerCase() && same(text, value))) return null;
      return { name: command, value };
    },
    labelOf(command) {
      if (this.playerAlias(command)) return command;
      const preset = bindPresets.presetOf(this.catalog, command);
      return preset ? this.text(preset.label) : command;
    },
    describe(key) {
      const bound = this.effective[key];
      const command = bound ? bound.command : "";
      const others = command
        ? (this.keysByCommand[bindPresets.commandKey(command)] || []).filter((other) => other !== key)
        : [];
      const word = command.split(" ")[0];
      const custom = this.playerAlias(command);
      const preset = command && !custom ? bindPresets.presetOf(this.catalog, command) : null;
      // Скрипт из заготовки лаунчер объявит сам, его alias в cfg игры может не быть.
      const alias = custom ? custom.value : command && !command.includes(" ") && !(preset && preset.aliases.length) ? this.aliasOf(word) : null;
      const runtime = (this.runtime[key] || []).map((item) => ({
        command: item.command,
        label: this.labelOf(item.command),
        file: item.file,
        trigger: keyName(item.trigger, this.$t),
      }));
      return {
        key,
        name: keyName(key, this.$t),
        command,
        label: command ? this.labelOf(command) : runtime.length ? runtime[0].label : "",
        tone: command ? this.toneOf(command) : runtime.length ? "runtime" : "none",
        runtime,
        note: preset && preset.note ? this.text(preset.note) : "",
        scriptWarning: !!command && this.scriptless && bindPresets.isScripted(this.catalog, command, this.aliases),
        practice: !!(preset && preset.practice),
        source: bound ? bound.source : null,
        overridden: !!(bound && bound.overridden) || this.isUnboundByLauncher(key),
        baseLabel: this.overrides[key] && this.overrides[key].base ? this.labelOf(this.overrides[key].base) : "",
        alias,
        customAlias: !!custom,
        otherKeys: others.map((other) => keyName(other, this.$t)),
      };
    },
    isUnboundByLauncher(key) {
      const entry = this.overrides[key];
      return !!entry && entry.command === "" && entry.base !== "";
    },
    select(key) {
      if (!key) return;
      this.selected = this.selected === key ? null : key;
      this.custom = "";
      this.customError = null;
    },
    baseOf(key) {
      return baseOf(key, this.fileBinds, this.overrides);
    },
    emit(next) {
      this.$emit("input", next);
    },
    assign(command) {
      if (!this.selected) return;
      const key = this.selected;
      const next = Object.assign({}, this.overrides);
      next[key] = { command: binds.cleanCommand(command), base: this.baseOf(key) };
      this.emit(next);
    },
    assignCustom() {
      const command = binds.cleanCommand(this.custom);
      const error = binds.commandError(command);
      if (error) {
        this.customError = this.$t(error === "TOO_LONG" ? "#L_KB_ERR_TOO_LONG" : "#L_KB_ERR_CHARS");
        return;
      }
      this.assign(command);
      this.custom = "";
    },
    // Отмена переназначения: запись остаётся «возвратом», чтобы игра снова
    // получила прежний бинд (config.cfg к этому времени уже хранит новый).
    revert() {
      if (!this.selected) return;
      const key = this.selected;
      const entry = this.overrides[key];
      if (!entry) return;
      const next = Object.assign({}, this.overrides);
      next[key] = { command: entry.base, base: entry.base };
      this.emit(next);
    },
  },
};
</script>

<style lang="scss" scoped>
.kb {
  --kb-blue: #4aa3ff;
  --kb-orange: #ff9f43;
  --kb-green: #5fd068;
  --kb-yellow: #f2cf4a;
  --kb-purple: #b48cff;
  --kb-pink: #ff7ac6;
  --kb-red: #ff6b6b;
  --kb-grey: #9aa4b5;
  --kb-other: #3fd0b0;
  --kb-alias: #d8b98a;
  --kb-runtime: #8fe3ff;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  // В маленьком окне схема и список действий не помещаются вместе:
  // тогда прокручивается вся вкладка, а не сжимается список.
  overflow-y: auto;
  overscroll-behavior: contain;
  color: var(--l-text);
  color-scheme: dark;
}

// Под панелью — разделитель (PaneSplitter): схема выше — клавиши крупнее, список заготовок ниже.
// Окно стало ниже — панель ужимается сама, нижней остаются её 190 px. min-height — строка легенды
// и 162 px схемы (28 px на клавишу): автоминимум панели — вся её высота, с ним она не ужималась бы.
// shortcut: легенда в две строки в самом тесном окне (масштаб 1,25 и выше, развёрнутая колонка) выйдет
// за панель на строку — если такое встретится, считать минимум по высоте легенды.
.kb-board-pane {
  flex: 0 1 auto;
  display: flex;
  flex-direction: column;
  min-height: 208px;
  // Ширина панели (cqw) ограничивает высоту схемы: выше, чем влезает по ширине, — незачем.
  container-type: inline-size;
  padding: 8px 12px 10px;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: var(--l-modal-bar);
}

.kb-legend {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 14px;
  min-height: 20px;
  margin-bottom: 6px;
  color: var(--l-text-dim);
  font-size: 11px;

  .kb-chip { padding: 2px 9px; }
  .kb-btn { min-height: 24px; padding: 2px 10px; }

  &__item { display: inline-flex; align-items: center; gap: 6px; }
  &__swatch { width: 10px; height: 10px; border-radius: 3px; background: var(--kb-c); }
  &__warn { width: 8px; height: 8px; border-radius: 2px; background: var(--kb-red); transform: rotate(45deg); }
  &__dot { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.08); }
  &__spacer { flex: 1; }
  &__note { color: var(--l-text-faint); }

}

// Высота схемы: с разделителя (--kb-h) или по умолчанию — прежняя ширина схемы в пересчёте на высоту.
// По умолчанию шире схема не нужна: клавиши только растут в высоту и отнимают место у списка.
// В низком окне схема уже, чтобы списку заготовок хватило места без
// прокрутки всей вкладки: 530px — окно, шапка страницы, подвал и список.
// Окно клавиатуры в настройках задаёт свой предел через --kb-fit.
// Не уже 28 px на клавишу: на Full HD (окно 648–676 px) иначе подписи 3–8 px.
.kb-board-box {
  flex: 0 1 auto;
  height: min(
    var(--kb-h, calc(var(--kb-fit, min(calc(var(--kb-cols) * 37px), max(calc(var(--kb-cols) * 28px), calc((100vh - 530px) * var(--kb-cols) / 5.8)))) * 5.8 / var(--kb-cols))),
    calc(100cqw * 5.8 / var(--kb-cols))
  );
  min-height: 162px;
  container-type: size;
}

// Схема вписывается в коробку по min(ширина, высота).
.kb-board {
  position: relative;
  width: min(100cqw, calc(100cqh * var(--kb-cols) / 5.8));
  margin: 0 auto;
  // Клавиши чуть шире, чем выше: так списку действий под схемой хватает места.
  aspect-ratio: var(--kb-cols) / 5.8;
  // Подписи клавиш считаются от ширины клавиши (cqi схемы / --kb-cols), а по высоте схемы
  // (size) низкие клавиши прячут бинд.
  container-type: size;
}

.kb-key {
  position: absolute;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  justify-content: space-between;
  // Снизу 6 px: полоса цвета занимает 2–5 px от низа, и бинд её не задевает.
  padding: 2px 4px 6px;
  border: 2px solid transparent;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.045);
  background-clip: padding-box;
  box-shadow: inset 0 -2px 0 rgba(0, 0, 0, 0.35), inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  color: var(--l-text-dim);
  font: inherit;
  text-align: left;
  overflow: hidden;
  cursor: pointer;
  transition: background var(--l-fast), box-shadow var(--l-fast);

  &::after {
    content: '';
    position: absolute;
    left: 4px;
    right: 4px;
    bottom: 2px;
    height: 3px;
    border-radius: 2px;
    background: var(--kb-c, transparent);
    opacity: 0.85;
  }

  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.09); color: var(--l-text); }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: -2px; }

  // Растут вместе с клавишей; при 37 px на клавишу — 9 и 7,8 px, как было.
  &__cap { font-size: clamp(9px, calc(100cqi / var(--kb-cols) * 0.25), 16px); font-weight: 600; line-height: 1; white-space: nowrap; }
  &__bind {
    width: 100%;
    color: var(--l-text);
    font-size: clamp(7.5px, calc(100cqi / var(--kb-cols) * 0.21), 13px);
    line-height: 1.1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  // Самое маленькое окно (1152×648): клавиша ниже 28 px, отступы тоньше.
  @container (max-width: 820px) {
    padding: 1px 3px 5px;
  }

  // Клавиша ниже 30 px: бинд не влезает читаемым (был 7,5 px и обрезан) —
  // остаётся полоса цвета, сам бинд виден при наведении в панели ниже.
  @container (max-height: 186px) {
    padding-inline: 2px;
    border-width: 1px;
    .kb-key__bind { display: none; }
  }

  &.is-bound {
    background: color-mix(in srgb, var(--kb-c) 14%, rgba(255, 255, 255, 0.04));
    color: var(--l-text);
  }
  // В Scriptless скрипт на клавише помечается красным ромбом.
  &__warn {
    position: absolute;
    top: 5px;
    right: 14px;
    width: 6px;
    height: 6px;
    border-radius: 1px;
    background: var(--kb-red);
    transform: rotate(45deg);
  }
  &.is-launcher::before {
    content: '';
    position: absolute;
    top: 4px;
    right: 4px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }
  &.is-selected {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 2px var(--accent);
    color: var(--l-text);
  }
  &.is-dead { opacity: 0.35; cursor: default; }
  // Клавишу назначает cfg по нажатию другой клавиши: пунктирная рамка, у пустой на старте —
  // ещё и пунктирная полоса своего цвета.
  &.is-runtime { border-style: dashed; border-color: color-mix(in srgb, var(--kb-runtime) 55%, transparent); }
  &.is-runtime:not(.is-bound) {
    background: color-mix(in srgb, var(--kb-runtime) 8%, rgba(255, 255, 255, 0.03));
    color: var(--l-text);
  }
  &.is-runtime:not(.is-bound)::after {
    background: repeating-linear-gradient(90deg, var(--kb-runtime) 0 4px, transparent 4px 7px);
  }
}

.kb-info__runtime { color: var(--kb-runtime); }

.tone-blue { --kb-c: var(--kb-blue); }
.tone-orange { --kb-c: var(--kb-orange); }
.tone-green { --kb-c: var(--kb-green); }
.tone-yellow { --kb-c: var(--kb-yellow); }
.tone-purple { --kb-c: var(--kb-purple); }
.tone-pink { --kb-c: var(--kb-pink); }
.tone-red { --kb-c: var(--kb-red); }
.tone-grey { --kb-c: var(--kb-grey); }
.tone-other { --kb-c: var(--kb-other); }
.tone-alias { --kb-c: var(--kb-alias); }
.tone-runtime { --kb-c: var(--kb-runtime); }



.kb-bottom {
  flex: 1 1 0;
  min-height: 190px;
  grid-template-rows: minmax(0, 1fr);
  display: grid;
  grid-template-columns: minmax(208px, 24%) minmax(0, 1fr);
  gap: 8px;
}

.kb-info, .kb-actions {
  min-height: 0;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: var(--l-modal-bar);
}

.kb-info {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  overflow-y: auto;

  &__head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  &__key {
    padding: 3px 9px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.08);
    box-shadow: inset 0 -2px 0 rgba(0, 0, 0, 0.35);
    font-size: 12px;
    font-weight: 600;
  }
  &__bind {
    margin: 0;
    padding-left: 10px;
    border-left: 3px solid var(--kb-c, rgba(255, 255, 255, 0.15));
    font-size: 13.5px;
    font-weight: 600;
    line-height: 1.3;
    overflow-wrap: anywhere;
  }
  &__code, &__note code {
    font-family: Consolas, monospace;
    font-size: 12px;
    color: var(--l-text-dim);
    overflow-wrap: anywhere;
  }
  &__note { margin: 0; color: var(--l-text-dim); font-size: 12px; line-height: 1.45; overflow-wrap: anywhere; }
  &__warn { margin: 0; color: #ff8a96; font-size: 12px; line-height: 1.45; }
  &__hint { margin: 0; color: var(--l-text-faint); font-size: 12px; line-height: 1.4; }
  &__hint--empty { margin: auto 0; text-align: center; }
  &__spacer { flex: 1; }
}

.kb-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 28px;
  padding: 3px 10px;
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
  transition: background var(--l-fast);

  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
  &:disabled { opacity: 0.45; cursor: default; }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
  i { font-size: 11px; }
  &--icon { width: 28px; justify-content: center; padding: 0; }
}

.kb-badge {
  padding: 2px 8px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.07);
  color: var(--l-text-dim);
  font-size: 11px;

  &.is-launcher { background: var(--accent-soft); color: var(--l-text); }
}

.kb-actions {
  display: flex;
  flex-direction: column;
  overflow: hidden;

  &__head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 6px 10px;
    border-bottom: 1px solid var(--l-border);
  }
  &__title { flex: 1; min-width: 160px; font-size: 13px; font-weight: 600; }
  &__error { margin: 8px 14px 0; color: #ff8a96; font-size: 12px; }
  &__empty { margin: 16px 0 0; color: var(--l-text-faint); font-size: 12px; text-align: center; }
  &__list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 2px 10px 8px;
  }
  &.is-idle .kb-actions__list { opacity: 0.5; }
}

.kb-custom {
  display: flex;
  gap: 8px;
  flex: 1 1 280px;

  &__input {
    flex: 1;
    min-width: 0;
    padding: 5px 10px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    color: var(--l-text);
    font-family: Consolas, monospace;
    font-size: 11.5px;

    &:focus { outline: none; border-color: var(--accent); }
    &.invalid { border-color: #ff4d5e; }
    &:disabled { opacity: 0.5; }
  }
}

.kb-group {
  padding-top: 4px;

  // Название группы стоит в одной строке с её заготовками.
  &__title {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-right: 4px;
    color: var(--l-text-dim);
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;

    &::before { content: ''; width: 7px; height: 7px; border-radius: 2px; background: var(--kb-c); }
  }
  &__items { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; }
}

.kb-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 7px;
  border: 1px solid var(--l-border);
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.035);
  color: var(--l-text);
  font: inherit;
  font-size: 11px;
  line-height: 1.3;
  cursor: pointer;
  transition: background var(--l-fast), border-color var(--l-fast);

  &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.08); }
  &:disabled { cursor: default; }
  &.active { border-color: var(--accent); background: var(--accent-soft); }
  &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
  b { font-weight: 600; }
  &__keys { color: var(--l-text-faint); font-size: 10px; }
  &__tag {
    padding: 0 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--kb-purple) 22%, transparent);
    color: var(--l-text-dim);
    font-size: 10px;
    line-height: 16px;

    &.is-practice { background: color-mix(in srgb, var(--kb-red) 22%, transparent); }
  }
}

.kb-modes {
  display: inline-flex;
  padding: 2px;
  border-radius: var(--l-radius-sm);
  box-shadow: inset 0 0 0 1px var(--l-border-strong);

  &__item {
    padding: 4px 10px;
    border: none;
    border-radius: calc(var(--l-radius-sm) - 2px);
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;

    &:hover { color: var(--l-text); }
    &.active { background: var(--accent-soft); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
  }
}

.kb-filter {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 5px 10px;
  border-bottom: 1px solid var(--l-border);
}

.kb-search {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 0 0 176px;
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
    padding: 4px 0;
    border: none;
    background: transparent;
    color: var(--l-text);
    font: inherit;
    font-size: 11.5px;

    &:focus { outline: none; }
  }
}

// Категории в одну строку; если не влезают, строка листается вбок
// (край тает — видно, что дальше есть ещё).
.kb-cats {
  display: flex;
  gap: 2px;
  flex: 1 1 0;
  min-width: 0;
  overflow-x: auto;
  scrollbar-width: none;
  padding-right: 24px;
  mask-image: linear-gradient(90deg, #000 calc(100% - 24px), transparent);

  &::-webkit-scrollbar { display: none; }

  &__item {
    display: inline-flex;
    align-items: center;
    flex: none;
    gap: 5px;
    padding: 2px 8px;
    border: 1px solid transparent;
    border-radius: 999px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 11px;
    white-space: nowrap;
    cursor: pointer;

    &[class*='tone-']::before { content: ''; width: 7px; height: 7px; border-radius: 2px; background: var(--kb-c); }
    &:hover { color: var(--l-text); background: rgba(255, 255, 255, 0.05); }
    &.active { border-color: var(--accent); background: var(--accent-soft); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 1px; }
  }
}

// Full HD (окно 648–676 px): схема читаемая, а нижняя панель целиком —
// строка «Назначить» в одну линию, список заготовок листается внутри.
@media (max-height: 700px) {
  .kb-bottom { min-height: 120px; }
  .kb-actions__head { flex-wrap: nowrap; padding-block: 5px; }
  .kb-actions__title { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .kb-custom { flex: 1 1 220px; min-width: 0; }
  .kb-filter { padding-block: 4px; }
}
</style>
